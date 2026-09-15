//! **Is `0x17` AUTO worth a second, and does the picture survive it?**
//!
//! `panel_deep_probe` found that this controller accepts AUTO — power on,
//! refresh and power off from one command — and that it completes in 14.943 s
//! where the hand-driven sequence takes 15.949 s for the same 14.45 s of
//! waveform. That is a second of pure sequencing overhead, on top of the
//! frame-rate win in [`docs/m5paper-refresh.md`], and it costs no accuracy at
//! all because it does not touch the waveform.
//!
//! One sample is not a measurement though, and a second saved is worth nothing
//! if the picture comes out wrong. So this interleaves the two paths three
//! times each at the PLL the firmware actually ships, and then draws the six-ink
//! test card through AUTO and leaves it, because whether the inks are right is
//! not a thing a timer can report.
//!
//! **The one real question is the booster retune.** M5GFX writes `0x06` with
//! `6F 1F 17 27` *after* `POWER_ON` on every refresh, and AUTO does its own
//! power-on with nowhere to put that write but before it. Both orders are
//! measured here; if they differ in time, they will differ in picture too.
//!
//! Run it with `tools/m5paper-build.sh autoprobe`.
//!
//! [`docs/m5paper-refresh.md`]: ../../../docs/m5paper-refresh.md

#![no_std]
#![no_main]
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

use geom::{FrameBuffer, Ink, Rotation, FB_BYTES};
use panel::{Ed2208, Refresh};

esp_bootloader_esp_idf::esp_app_desc!();

static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// The PLL the firmware ships for gameplay, so the comparison is against the
/// sequence that actually runs rather than the vendor's.
const PLL: u8 = 0x07;

const ROUNDS: usize = 3;

#[main]
fn main() -> ! {
    let p = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()),
    );
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: 0x17 AUTO against the hand-driven sequence ===");
    println!("interleaved, {ROUNDS} rounds each, at PLL 0x{PLL:02X}.");

    let mut i2c = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("i2c init")
    .with_sda(p.GPIO3)
    .with_scl(p.GPIO2);
    print!("pmic      : enabling panel power... ");
    match pmic::enable_panel_power(&mut i2c, &delay) {
        Ok(()) => println!("ok"),
        Err(e) => println!("FAILED: {:?}", e),
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

    print!("panel     : reset + init... ");
    epd.reset();
    println!("{}", if epd.init() { "ok" } else { "TIMED OUT" });
    epd.set_pll(PLL);

    // SAFETY: single task, and this is the only reference taken to FB.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
    let mut fb = FrameBuffer::new(buf, Rotation::Landscape);

    println!();
    println!("--- interleaved, so a drifting panel cannot favour either path ---");
    let (mut hand, mut auto_plain, mut auto_tuned) = (0u64, 0u64, 0u64);
    for r in 0..ROUNDS {
        fb.clear(if r % 2 == 0 { Ink::Black } else { Ink::White });
        epd.send_framebuffer(&fb);
        let a = epd.refresh_full();
        line("hand-driven", a);
        hand += a.total_ms;

        fb.clear(if r % 2 == 0 { Ink::White } else { Ink::Black });
        epd.send_framebuffer(&fb);
        let b = epd.refresh_auto(false, &mut || {});
        line("AUTO, no retune", b);
        auto_plain += b.total_ms;

        fb.clear(if r % 2 == 0 { Ink::Black } else { Ink::White });
        epd.send_framebuffer(&fb);
        let c = epd.refresh_auto(true, &mut || {});
        line("AUTO + booster retune", c);
        auto_tuned += c.total_ms;
    }

    let n = ROUNDS as u64;
    println!();
    println!("--- means over {ROUNDS} rounds ---");
    mean("hand-driven", hand / n, hand / n);
    mean("AUTO, no retune", auto_plain / n, hand / n);
    mean("AUTO + booster retune", auto_tuned / n, hand / n);

    // The picture, through AUTO, left where it can be judged. Colour separation
    // is what a mis-sequenced power-up would cost, so the six-ink card is the
    // check rather than a flat fill.
    println!();
    println!("--- the test card through AUTO + retune, left on the glass ---");
    geom::draw_test_card(&mut fb);
    epd.send_framebuffer(&fb);
    let r = epd.refresh_auto(true, &mut || {});
    line("test card via AUTO", r);

    println!();
    println!("LOOK AT THE PANEL. six ink bands, each boxed in black:");
    println!("  black, white, yellow, red, blue, green — distinct and solid.");
    println!("  compare against: tools/m5paper-build.sh testcard");
    println!("this card was drawn by AUTO at PLL 0x{PLL:02X}. if it matches the host");
    println!("render, AUTO is free and the firmware should use it. if the inks are");
    println!("weaker than the hand-driven path's, the second it saves is not free.");
    loop {
        delay.delay_millis(1000);
    }
}

fn line(what: &str, r: Refresh) {
    print!("  {what:26}");
    if !r.saw_busy {
        println!("REFUSED (BUSY never went low)");
        return;
    }
    if !r.settled {
        println!("TIMED OUT after {}.{:03} s", r.drf_ms / 1000, r.drf_ms % 1000);
        return;
    }
    println!(
        "waveform {}.{:03} s   total {}.{:03} s",
        r.drf_ms / 1000,
        r.drf_ms % 1000,
        r.total_ms / 1000,
        r.total_ms % 1000
    );
}

fn mean(what: &str, ms: u64, against: u64) {
    let d = ms as i64 - against as i64;
    println!(
        "  {what:26}{}.{:03} s   {d:+} ms",
        ms / 1000,
        ms % 1000
    );
}

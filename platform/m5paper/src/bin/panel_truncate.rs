//! **Is half a waveform legible?**
//!
//! The one experiment on this panel whose answer is a picture rather than a
//! number, so it is driven by the buttons and judged by whoever is holding it.
//!
//! A refresh is long because the Spectra 6 waveform separates six pigments, and
//! that length is not a register — it is how long the pigments get to move.
//! Nothing here asks the controller for a shorter waveform. It starts the
//! normal one and cuts the power part-way, which needs no undocumented command
//! and no cooperation from the controller at all. The pigments end up wherever
//! they had got to.
//!
//! **If a two-second cut is readable in black and white, that is four times
//! again on top of the frame-rate win**, and the first thing on this device
//! that could be called responsive. If it is a grey smear, the honest answer is
//! that the waveform is the wall and the 8.741 s in
//! [`docs/m5paper-refresh.md`] is the floor.
//!
//! # Using it
//!
//! Each step draws the six-ink test card, so the thing to watch is how many of
//! the six bands have arrived and whether black and white have separated before
//! the colours do.
//!
//! - **A** — next cut, longer. **B** — previous cut, shorter.
//! - **C** — the same card at the vendor's full waveform, as the reference.
//!
//! Every step first drives the panel white with a full, stock refresh, so each
//! cut starts from the same place and is comparing against the same history
//! rather than against the last experiment's residue. That costs sixteen
//! seconds a step and is the only way the comparison means anything.
//!
//! # What it can cost
//!
//! Cutting power mid-waveform leaves charge on the pigments that the waveform
//! would have resolved, which is what ghosting is. **Every step ends by
//! returning to a full stock refresh before the next one**, which is the same
//! thing a normal update does and is how e-paper clears residue. The captain
//! authorised this on his own device; it is still worth pressing C at the end
//! and leaving the glass on a clean, complete picture.
//!
//! [`docs/m5paper-refresh.md`]: ../../../docs/m5paper-refresh.md
//!
//! Run it with `tools/m5paper-build.sh truncate`.

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
use esp_hal::gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::main;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::{print, println};

use geom::{FrameBuffer, Ink, Rotation, FB_BYTES};
use panel::Ed2208;

esp_bootloader_esp_idf::esp_app_desc!();

static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// Where to cut, in milliseconds of a waveform that runs 7.251 s at PLL 0x07.
///
/// The ladder is denser at the short end because that is where the answer
/// changes: if anything is legible at all it will be black and white first, and
/// the colours last.
const CUTS: [u64; 7] = [500, 1_000, 1_500, 2_000, 3_000, 4_500, 6_000];

/// The PLL every cut runs at. The fast one, because a cut is only interesting
/// against the shortest honest waveform rather than the vendor's.
const CUT_PLL: u8 = 0x07;

#[main]
fn main() -> ! {
    let p = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()),
    );
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: how much waveform does a legible picture need? ===");

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
        Err(e) => println!("FAILED: {:?} — the panel is unpowered.", e),
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

    // SAFETY: single task, and this is the only reference taken to FB.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
    let mut fb = FrameBuffer::new(buf, Rotation::Landscape);

    let mut buttons = [
        ("A", Flex::new(p.GPIO10)),
        ("B", Flex::new(p.GPIO9)),
        ("C", Flex::new(p.GPIO1)),
    ];
    for (_, pin) in buttons.iter_mut() {
        pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
        pin.set_input_enable(true);
    }

    println!();
    println!("A = longer cut   B = shorter cut   C = the full waveform, as reference");
    println!("each step drives the panel white at the vendor's settings first, so");
    println!("every cut starts from the same picture. that is the sixteen seconds.");
    println!("watch which inks arrive first: black and white should beat the colours.");

    let mut ix = 0usize;
    step(&mut epd, &mut fb, CUTS[ix]);

    let mut was = [false; 3];
    loop {
        for i in 0..buttons.len() {
            let down = buttons[i].1.is_low();
            if down && !was[i] {
                println!();
                match i {
                    0 => {
                        ix = (ix + 1).min(CUTS.len() - 1);
                        step(&mut epd, &mut fb, CUTS[ix]);
                    }
                    1 => {
                        ix = ix.saturating_sub(1);
                        step(&mut epd, &mut fb, CUTS[ix]);
                    }
                    _ => reference(&mut epd, &mut fb),
                }
            }
            was[i] = down;
        }
        delay.delay_millis(20);
    }
}

/// White at the vendor's settings, then the test card cut at `ms`.
fn step<Dm: esp_hal::DriverMode>(epd: &mut Ed2208<'_, Dm>, fb: &mut FrameBuffer<'_>, ms: u64) {
    println!("clearing to white at the vendor's PLL 0x08 (~16 s)...");
    epd.set_pll(0x08);
    fb.clear(Ink::White);
    epd.send_framebuffer(fb);
    let _ = epd.refresh_full();

    println!("test card at PLL 0x{CUT_PLL:02X}, power cut at {ms} ms of a 7.251 s waveform...");
    epd.set_pll(CUT_PLL);
    geom::draw_test_card(fb);
    epd.send_framebuffer(fb);
    let r = epd.refresh_truncated(ms);
    if r.settled {
        println!("  the waveform had already finished — this cut is not a cut.");
    } else {
        println!(
            "  cut at {}.{:03} s. LOOK: how many of the six bands are there?",
            r.drf_ms / 1000,
            r.drf_ms % 1000
        );
    }
    // An interrupted waveform leaves the controller mid-sequence; reset and
    // re-init rather than issuing the next command into whatever state that is.
    epd.reset();
    epd.init();
}

/// The same card at the full waveform, as the thing to compare against.
fn reference<Dm: esp_hal::DriverMode>(epd: &mut Ed2208<'_, Dm>, fb: &mut FrameBuffer<'_>) {
    println!("reference: the test card at the vendor's full waveform (~16 s)...");
    epd.set_pll(0x08);
    geom::draw_test_card(fb);
    epd.send_framebuffer(fb);
    let r = epd.refresh_full();
    println!(
        "  {}.{:03} s. this is what a complete waveform looks like.",
        r.total_ms / 1000,
        r.total_ms % 1000
    );
}

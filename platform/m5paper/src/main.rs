//! First boot on the M5Stack PaperColor: prove the toolchain, the panel and a
//! button, in that order.
//!
//! This is deliberately not an emulator port. Three things can sink that port
//! independently — the forked Xtensa toolchain, this specific panel, and the
//! PSRAM atomics hazard — and this binary settles the first two while touching
//! none of TerminalGB's own code. See platform/m5paper/README.md.
//!
//! Note it never allocates from PSRAM. The framebuffer is a 120 KB static in
//! internal SRAM, so the `s32c1i`-on-PSRAM atomics hazard cannot be in play
//! here; that is a question for the port, not for bring-up.

#![no_std]
#![no_main]

mod geom;
mod panel;
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

use geom::{draw_test_card, FrameBuffer, Ink, Rotation, FB_BYTES};
use panel::Ed2208;

esp_bootloader_esp_idf::esp_app_desc!();

/// The panel framebuffer. 120,000 bytes, internal SRAM, never PSRAM.
static mut FRAMEBUFFER: [u8; FB_BYTES] = [0; FB_BYTES];

/// The orientation the captain asked for. The panel is physically portrait;
/// everything this binary draws is mapped into landscape on the way in.
const ORIENTATION: Rotation = Rotation::Landscape;

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!();
    println!("=== TerminalGB / M5PaperColor first boot ===");
    println!("chip      : ESP32-S3, Xtensa LX7");
    println!("built with: espup Xtensa Rust fork (upstream rustc cannot target this)");
    println!(
        "panel     : {}x{} native, {}x{} logical ({:?})",
        geom::PANEL_W,
        geom::PANEL_H,
        ORIENTATION.width(),
        ORIENTATION.height(),
        ORIENTATION
    );

    // --- 1. Panel power, which lives behind the PMIC, not behind a GPIO ------
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
            println!("          the panel is unpowered; every SPI write below is a no-op.");
        }
    }

    // --- 2. Panel bus -------------------------------------------------------
    // GPIO44 is chip-select and must be high before anything else touches the
    // bus. GPIO43 is data/command. GPIO12 is the real reset line.
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
        println!("TIMED OUT waiting on BUSY (GPIO11)");
        println!("          panel is powered but not answering, or power failed above.");
    }

    // SAFETY: single-threaded, single core in use, and this is the only
    // reference taken to the framebuffer for the lifetime of the program.
    let fb_ref = unsafe { &mut *core::ptr::addr_of_mut!(FRAMEBUFFER) };
    let mut fb = FrameBuffer::new(fb_ref, ORIENTATION);

    // --- 3. One colour, timed ----------------------------------------------
    // The brief's actual ask, and the cleanest thing to time: a flat fill is
    // the easiest picture this panel can be given.
    println!();
    println!("--- refresh 1: solid {} ---", Ink::Red.name());
    fb.clear(Ink::Red);
    epd.send_framebuffer(&fb);
    let ms = epd.refresh();
    println!("solid {:>6}: {}.{:03} s", Ink::Red.name(), ms / 1000, ms % 1000);

    // --- 4. A picture that proves the orientation ---------------------------
    // A flat colour cannot show which way up the panel is, so the second
    // refresh draws something asymmetric in both axes.
    println!();
    println!("--- refresh 2: landscape test card ---");
    draw_test_card(&mut fb);
    epd.send_framebuffer(&fb);
    let ms = epd.refresh();
    println!("test card  : {}.{:03} s", ms / 1000, ms % 1000);

    // --- 5. Buttons ---------------------------------------------------------
    // Three user buttons, active low, wired to ground with an internal pull-up.
    // `Flex` rather than `Input` so the read path can be self-tested below.
    let mut buttons = [
        ("A", 10u8, Flex::new(p.GPIO10)),
        ("B", 9, Flex::new(p.GPIO9)),
        ("C", 1, Flex::new(p.GPIO1)),
    ];
    for (_, _, pin) in buttons.iter_mut() {
        pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
        pin.set_input_enable(true);
    }

    println!();
    println!("--- buttons ---");
    for (name, gpio, pin) in buttons.iter() {
        println!(
            "  {} = GPIO{:<2} idle level {} (1 = released)",
            name,
            gpio,
            pin.is_high() as u8
        );
    }

    // Electrical self-test of the *read* path only. Driving the pin low from
    // the inside is safe (the button grounds it too) and proves the GPIO can be
    // read and that a low is seen. It does NOT prove a button is physically
    // attached to that pin: only a finger can do that.
    //
    // The pull-up must be re-applied afterwards. `OutputConfig::default()`
    // carries `Pull::None`, so applying it drops the pull-up that `Input` had
    // set - which leaves the pin floating, and a floating pin reads as a
    // permanently-held button. That cost a false "button C pressed" the first
    // time this ran.
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    print!("read-path self-test: ");
    let mut all_ok = true;
    for (name, _, pin) in buttons.iter_mut() {
        pin.apply_output_config(&OutputConfig::default());
        pin.set_low();
        pin.set_output_enable(true);
        delay.delay_millis(5);
        let pulled_low = pin.is_low();

        pin.set_output_enable(false);
        pin.apply_input_config(&pull_up); // restore, or the pin floats
        pin.set_input_enable(true);
        // 5 ms, not microseconds: GPIO1 rises through the ~45k internal pull-up
        // far more slowly than GPIO9/10, which is the signature of a hardware
        // debounce capacitor on that button. At 500 us it read as still-low and
        // looked like a fault.
        delay.delay_millis(5);
        let released_high = pin.is_high();

        if !(pulled_low && released_high) {
            all_ok = false;
            print!("{}: low={} high={} ", name, pulled_low as u8, released_high as u8);
        }
    }
    if all_ok {
        println!("ok (all three read both levels)");
    } else {
        println!("<- FAILED");
    }

    // Levels again after the self-test: if these do not match the idle levels
    // printed above, the self-test disturbed the pins and nothing below is
    // trustworthy.
    print!("idle levels after self-test:");
    for (name, _, pin) in buttons.iter() {
        print!(" {}={}", name, pin.is_high() as u8);
    }
    println!();
    println!("press a button: the panel repaints in the next ink.");

    let mut was = [false; 3];
    let mut next_ink = 0usize;
    loop {
        for i in 0..buttons.len() {
            let down = buttons[i].2.is_low();
            if down && !was[i] {
                let ink = Ink::ALL[next_ink % Ink::ALL.len()];
                next_ink += 1;
                println!(
                    "button {} (GPIO{}) pressed -> repainting {}",
                    buttons[i].0, buttons[i].1, ink.name()
                );
                fb.clear(ink);
                epd.send_framebuffer(&fb);
                let ms = epd.refresh();
                println!("  {:>6}: {}.{:03} s", ink.name(), ms / 1000, ms % 1000);
            }
            was[i] = down;
        }
        delay.delay_millis(20);
    }
}

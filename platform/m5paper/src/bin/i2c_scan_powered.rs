//! Same I2C scan as `i2c_scan.rs`, but first raises every M5PM1 GPIO bit
//! `src/pmic.rs` does NOT already use (0 = panel, 3 = SD — those stay as
//! `enable_panel_power` leaves them). The panel itself needed exactly this
//! before it would answer *anything* on SPI; if the codec's power rail is
//! gated the same way, off it is indistinguishable from absent on a plain
//! scan, and raising the other bits first is the direct test rather than
//! reading a datasheet nobody here has for this specific PMIC.
//!
//! Bits 1, 2, 4, 5, 6, 7 are set as push-pull outputs, driven high, then
//! both buses (`i2c_scan.rs`'s own two) are scanned again.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::main;
use esp_hal::time::Rate;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

const PMIC_ADDR: u8 = 0x6E;
const REG_GPIO_MODE: u8 = 0x10;
const REG_GPIO_OUT: u8 = 0x11;
const REG_GPIO_PP: u8 = 0x13;
const REG_GPIO_FUNC: u8 = 0x16;
/// Bits 0 (panel) and 3 (SD) are already claimed by `pmic::enable_panel_power`
/// and stay whatever that leaves them; every other bit is fair game.
const OTHER_BITS: u8 = 0b1101_0110; // bits 1,2,4,5,6,7

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: raise every unclaimed PMIC GPIO, then scan I2C ===");

    let mut pmic_i2c = match I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100))) {
        Ok(i) => i.with_scl(p.GPIO2).with_sda(p.GPIO3),
        Err(e) => {
            println!("pmic i2c init failed: {:?}", e);
            loop {
                delay.delay_millis(10_000);
            }
        }
    };

    macro_rules! rmw {
        ($reg:expr, $data:expr, $mask:expr) => {{
            let mut buf = [0u8; 1];
            match pmic_i2c.write_read(PMIC_ADDR, &[$reg], &mut buf) {
                Ok(()) => {
                    let next = (buf[0] & $mask) | $data;
                    let _ = pmic_i2c.write(PMIC_ADDR, &[$reg, next]);
                }
                Err(e) => println!("pmic read of 0x{:02X} failed: {:?}", $reg, e),
            }
        }};
    }

    // The function-select register only has 2 bits per pin for pins 0..3
    // in one byte (M5GFX's own use of it never goes past bit 3), so this
    // clears the field for whichever of our target bits fall in that range
    // and leaves the rest alone rather than computing an 8-bit shift that
    // does not fit.
    rmw!(REG_GPIO_FUNC, 0x00, !((0b11 << 2) | (0b11 << 4)));
    rmw!(REG_GPIO_MODE, OTHER_BITS, !OTHER_BITS);
    rmw!(REG_GPIO_PP, 0x00, !OTHER_BITS);
    rmw!(REG_GPIO_OUT, OTHER_BITS, !OTHER_BITS);
    println!("pmic      : bits 1,2,4,5,6,7 set output push-pull, driven high");
    delay.delay_millis(50);

    println!();
    println!("bus       : internal (GPIO2 SCL / GPIO3 SDA)");
    scan(&mut pmic_i2c, &delay);

    drop(pmic_i2c);
    println!();
    println!("bus       : external Port A (GPIO5 SCL / GPIO4 SDA)");
    match I2c::new(p.I2C1, I2cConfig::default().with_frequency(Rate::from_khz(100))) {
        Ok(i2c) => {
            let mut i2c = i2c.with_scl(p.GPIO5).with_sda(p.GPIO4);
            scan(&mut i2c, &delay);
        }
        Err(e) => println!("          : could not init — {:?}", e),
    }

    println!();
    println!("done.");
    loop {
        delay.delay_millis(10_000);
    }
}

fn scan<Dm: esp_hal::DriverMode>(i2c: &mut I2c<'_, Dm>, delay: &Delay) {
    let mut found = 0u32;
    for addr in 0x08u8..=0x77 {
        if i2c.write(addr, &[]).is_ok() {
            let note = match addr {
                0x18 | 0x19 => " <- ES8311's usual address",
                0x6E => " (PMIC)",
                0x32 => " (RX8130 RTC)",
                0x44 => " (SHT40)",
                _ => "",
            };
            println!("          : 0x{addr:02X} responded{note}");
            found += 1;
        }
        delay.delay_micros(200);
    }
    if found == 0 {
        println!("          : nothing responded on this bus");
    } else {
        println!("          : {found} device(s) found on this bus");
    }
}

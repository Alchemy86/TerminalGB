//! The one thing the last two probes did NOT try: raising bit 0 (the
//! panel's own power rail) before scanning. Both earlier probes deliberately
//! left bits 0 and 3 alone to avoid disturbing `pmic::enable_panel_power`'s
//! own bits — which means neither of them tested the one rail this board is
//! *known* to gate by GPIO (the panel), and if the audio codec shares that
//! rail rather than its own, both earlier runs would show it as absent
//! whether or not it is there. This raises every bit — panel, SD, and the
//! six spare ones — then scans both buses.

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
const REG_WDT_CNT: u8 = 0x0A;
/// Every bit this PMIC exposes.
const ALL_BITS: u8 = 0xFF;

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: EVERY PMIC GPIO high (panel included), then scan I2C ===");

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

    rmw!(REG_WDT_CNT, 0x00, 0x00);
    // Function-select is 2 bits per pin and this register only covers
    // pins 0..3 in one byte (same as pmic::enable_panel_power).
    rmw!(REG_GPIO_FUNC, 0x00, 0x00);
    rmw!(REG_GPIO_MODE, ALL_BITS, 0x00);
    rmw!(REG_GPIO_PP, 0x00, 0x00);
    rmw!(REG_GPIO_OUT, ALL_BITS, 0x00);
    println!("pmic      : every GPIO bit set output push-pull, driven high — panel powered too");
    delay.delay_millis(150);

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

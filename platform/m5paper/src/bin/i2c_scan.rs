//! **Does this board actually have an ES8311 on it?** A plain I2C scan of
//! both buses this chip could be on, because the open-source M5Unified board
//! table for `board_M5PaperColor` never wires one up — which is either a gap
//! in that table for this hardware revision, or the codec is on a bus the
//! table doesn't name. A scan settles it either way, on the real board,
//! rather than by reading someone else's source a second time.
//!
//! Scans the internal bus (GPIO2 SCL / GPIO3 SDA — the one `src/pmic.rs`
//! already talks to the M5PM1 on) and the external "Port A" pins M5Unified's
//! own pin table lists for this board (GPIO5 SCL / GPIO4 SDA), at 100 kHz,
//! addresses `0x08..=0x77` (the usual scan range — `0x00..=0x07` and
//! `0x78..=0x7F` are reserved). ES8311's datasheet default address is
//! `0x18`; AW8737-class amps are usually a plain GPIO enable, not an I2C
//! device, so this cannot see the amp directly — only the codec.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::main;
use esp_hal::time::Rate;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: I2C scan, looking for an ES8311 ===");
    println!("(the panel's power rail is left alone — nothing here touches the PMIC)");
    println!();

    println!("bus       : internal (GPIO2 SCL / GPIO3 SDA — the PMIC's own bus)");
    match I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100))) {
        Ok(i2c) => {
            let mut i2c = i2c.with_scl(p.GPIO2).with_sda(p.GPIO3);
            scan(&mut i2c, &delay);
        }
        Err(e) => println!("          : could not init — {:?}", e),
    }

    println!();
    println!("bus       : external Port A (GPIO5 SCL / GPIO4 SDA — M5Unified's second table)");
    match I2c::new(p.I2C1, I2cConfig::default().with_frequency(Rate::from_khz(100))) {
        Ok(i2c) => {
            let mut i2c = i2c.with_scl(p.GPIO5).with_sda(p.GPIO4);
            scan(&mut i2c, &delay);
        }
        Err(e) => println!("          : could not init — {:?}", e),
    }

    println!();
    println!("done. 0x18 or 0x19 responding is an ES8311; nothing on either bus");
    println!("means the codec is not reachable from software as these two pin");
    println!("pairs currently configure it — the next thing to try is whichever");
    println!("other GPIO pair is free on this board.");
    loop {
        delay.delay_millis(10_000);
    }
}

fn scan<Dm: esp_hal::DriverMode>(i2c: &mut I2c<'_, Dm>, delay: &Delay) {
    let mut found = 0u32;
    for addr in 0x08u8..=0x77 {
        // A zero-length write is the standard scan probe: it only asks
        // whether the device acknowledges its address, and touches no
        // register on whatever answers.
        if i2c.write(addr, &[]).is_ok() {
            let note = match addr {
                0x18 | 0x19 => " <- ES8311's usual address",
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

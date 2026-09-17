//! The missing piece from the last three probes: `AUDIO_PWR_EN` is **not** a
//! PMIC bit at all — it is a plain ESP32-S3 GPIO, **G45**, wired straight to
//! the ES8311/ES7210's own 3V3 rail (net `CODEC_3V3_L3B`), per M5Stack's own
//! PaperColor pinout page. Every earlier probe toggled the M5PM1's I2C
//! registers and never touched G45 directly, so the codec was never powered
//! in any of them — a scan of an unpowered chip and a scan of an absent one
//! look identical, which is exactly the same trap the panel's own PMIC gate
//! already taught this port once.
//!
//! Drives G45 high, waits for the rail to settle, then scans the audio I2C
//! bus (G2 SCL / G3 SDA — the same bus the PMIC, RTC and SHT40 are already
//! on) for ES8311 (0x18) and ES7210 (0x40) specifically, and reports the
//! whole 0x08..0x77 range for anything else that shows up once the rail is
//! live.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
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
    println!("=== M5PaperColor: G45 (AUDIO_PWR_EN) high, then scan for ES8311/ES7210 ===");

    let audio_pwr_en = Output::new(p.GPIO45, Level::High, OutputConfig::default());
    println!("gpio45    : AUDIO_PWR_EN driven high — codec/mic rail should be live");
    delay.delay_millis(50);

    match I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100))) {
        Ok(i2c) => {
            let mut i2c = i2c.with_scl(p.GPIO2).with_sda(p.GPIO3);
            println!();
            println!("bus       : audio I2C (G2 SCL / G3 SDA)");
            let mut found = 0u32;
            for addr in 0x08u8..=0x77 {
                if i2c.write(addr, &[]).is_ok() {
                    let note = match addr {
                        0x18 => " <- ES8311",
                        0x40 => " <- ES7210",
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
            println!("          : {found} device(s) found on this bus");
        }
        Err(e) => println!("i2c init failed: {:?}", e),
    }

    println!();
    println!("done. leaving G45 high.");
    core::mem::forget(audio_pwr_en); // keep the rail driven for as long as this firmware runs
    loop {
        delay.delay_millis(10_000);
    }
}

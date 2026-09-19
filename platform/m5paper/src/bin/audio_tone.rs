//! **The first sound out of this board, ever.** A plain sine tone through
//! the real chain: `AUDIO_PWR_EN` (G45) powers the ES8311, an I2C sequence
//! brings the codec up, `SPK_EN` (G46) enables the AW8737A amp, and I2S
//! (MCLK G42 / BCLK G40 / WS G41 / DOUT G39) streams the samples. Every pin
//! is from M5Stack's own PaperColor pinout page; every I2S call is from
//! esp-hal's own `embassy-i2s-sound` example, adapted to blocking mode since
//! this crate has no async executor anywhere else.
//!
//! **The ES8311 register sequence is borrowed, not derived, and it is the
//! one part of this that is a real guess.** It is M5Unified's own bring-up
//! for a *different* M5Stack board (their internal name is literally
//! `chain_maintainer`) that happens to carry the same ES8311 part — reset, bring
//! up the clock manager, power the analog block and the DAC, enable the
//! headphone drive, set volume, bypass the DAC's own equalizer. That board's
//! codec may derive its master clock differently than this one does (its
//! `CLOCK_MANAGER` register hints at deriving the clock from BCLK rather
//! than from a wired MCLK pin, and PaperColor's own pinout gives the codec a
//! real MCLK trace), so the pitch or level here may not be exactly right —
//! but if the chain works at all, something audible comes out, which is the
//! thing nobody here can check without ears in the room.
//!
//! The sine table is esp-hal's own (MIT/Apache-2.0), a plain 64-sample table,
//! nothing decompressed or extracted from anywhere.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::i2s::master::{Channels, DataFormat, I2s, TdmConfig};
use esp_hal::main;
use esp_hal::time::Rate;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

const ES8311_ADDR: u8 = 0x18;
/// (register, value) pairs, in order. See the module docs for where these
/// come from and the one real caveat.
const ES8311_INIT: &[(u8, u8)] = &[
    (0x00, 0x80), // RESET: CSM power on
    (0x01, 0xB5), // CLOCK_MANAGER: MCLK=BCLK
    (0x02, 0x18), // CLOCK_MANAGER: MULT_PRE=3
    (0x0D, 0x01), // SYSTEM: power up analog circuitry
    (0x12, 0x00), // SYSTEM: power up DAC
    (0x13, 0x10), // SYSTEM: enable output to HP drive
    (0x32, 0xEF), // DAC: volume (0xBF == +-0 dB; this is louder)
    (0x37, 0x08), // DAC: bypass DAC equaliser
];

/// esp-hal's own `embassy-i2s-sound` example table (MIT/Apache-2.0): one
/// cycle of a 16-bit signed sine wave, nothing to do with any cartridge.
const SINE: [i16; 64] = [
    0, 3211, 6392, 9511, 12539, 15446, 18204, 20787, 23169, 25329, 27244, 28897, 30272, 31356,
    32137, 32609, 32767, 32609, 32137, 31356, 30272, 28897, 27244, 25329, 23169, 20787, 18204,
    15446, 12539, 9511, 6392, 3211, 0, -3211, -6392, -9511, -12539, -15446, -18204, -20787, -23169,
    -25329, -27244, -28897, -30272, -31356, -32137, -32609, -32767, -32609, -32137, -31356, -30272,
    -28897, -27244, -25329, -23169, -20787, -18204, -15446, -12539, -9511, -6392, -3211,
];

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: first sound — a plain sine tone ===");

    let _audio_pwr_en = Output::new(p.GPIO45, Level::High, OutputConfig::default());
    println!("gpio45    : AUDIO_PWR_EN high");
    delay.delay_millis(50);

    let mut i2c = match I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100))) {
        Ok(i) => i.with_scl(p.GPIO2).with_sda(p.GPIO3),
        Err(e) => {
            println!("i2c init failed: {:?}", e);
            loop {
                delay.delay_millis(10_000);
            }
        }
    };

    print_es8311_present(&mut i2c);

    for &(reg, val) in ES8311_INIT {
        match i2c.write(ES8311_ADDR, &[reg, val]) {
            Ok(()) => println!("es8311    : reg 0x{reg:02X} <- 0x{val:02X}"),
            Err(e) => println!("es8311    : reg 0x{reg:02X} write FAILED: {:?}", e),
        }
        delay.delay_millis(2);
    }

    drop(i2c);

    let _spk_en = Output::new(p.GPIO46, Level::High, OutputConfig::default());
    println!("gpio46    : SPK_EN high (AW8737A amplifier enabled)");
    delay.delay_millis(10);

    let dma_channel = p.DMA_CH0;
    let mut tx_buf = match esp_hal::dma_tx_buffer!(4096) {
        Ok(b) => b,
        Err(e) => {
            println!("dma buffer failed: {:?}", e);
            loop {
                delay.delay_millis(10_000);
            }
        }
    };

    // Fill it with the sine table repeated, stereo (each i16 sample doubled
    // into left+right), which is what `Channels::STEREO` + `Data16Channel16`
    // expects on the wire.
    {
        let cap = tx_buf.capacity();
        let slice = tx_buf.as_mut_slice();
        let mut idx = 0usize;
        let mut pos = 0usize;
        while pos + 2 <= cap {
            let sample = SINE[idx % SINE.len()];
            let bytes = sample.to_le_bytes();
            slice[pos] = bytes[0];
            slice[pos + 1] = bytes[1];
            pos += 2;
            // advance the sine index every OTHER 16-bit slot, so left and
            // right carry the same sample rather than every slot advancing.
            if pos % 4 == 0 {
                idx += 1;
            }
        }
        tx_buf.set_length(cap);
    }

    let i2s = match I2s::new(
        p.I2S0,
        dma_channel,
        TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(44100))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO),
    ) {
        Ok(i) => i,
        Err(e) => {
            println!("i2s init failed: {:?}", e);
            loop {
                delay.delay_millis(10_000);
            }
        }
    };
    // `with_mclk` is on the whole `I2s`, before it splits into tx/rx halves —
    // ESP32-S3 needs it explicit (esp-hal's own doc only skips this on the
    // original ESP32, which generates MCLK differently).
    let i2s = i2s.with_mclk(p.GPIO42);

    let i2s_tx = i2s
        .i2s_tx
        .with_bclk(p.GPIO40)
        .with_ws(p.GPIO41)
        .with_dout(p.GPIO39)
        .build();

    println!("i2s       : starting, 44100 Hz stereo, ~440 Hz tone, on repeat");
    let mut tx = i2s_tx;
    let mut round = 0u32;
    loop {
        match tx.write(tx_buf) {
            Ok(transfer) => {
                let (result, returned_tx, returned_buf) = transfer.wait();
                if let Err(e) = result {
                    println!("i2s       : transfer error: {:?}", e);
                }
                tx = returned_tx;
                tx_buf = returned_buf;
                round += 1;
                if round % 20 == 0 {
                    println!("i2s       : {round} buffers sent");
                }
            }
            Err((e, returned_tx, returned_buf)) => {
                println!("i2s       : write failed: {:?}", e);
                tx = returned_tx;
                tx_buf = returned_buf;
                delay.delay_millis(100);
            }
        }
    }
}

fn print_es8311_present<Dm: esp_hal::DriverMode>(i2c: &mut I2c<'_, Dm>) {
    match i2c.write(ES8311_ADDR, &[]) {
        Ok(()) => println!("es8311    : present at 0x{ES8311_ADDR:02X}"),
        Err(_) => println!("es8311    : NOT responding at 0x{ES8311_ADDR:02X} — nothing will be heard"),
    }
}

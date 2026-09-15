//! The M5PM1 power-management chip, which gates power to the e-paper panel.
//!
//! This is the part of the board that is not guessable from a schematic photo:
//! the panel has no GPIO power-enable line of its own. It is switched by a
//! separate PMIC over I2C, and until that PMIC is told to raise its own GPIO0
//! the panel is unpowered and every SPI command is written into the void with
//! no error of any kind.
//!
//! Sequence and register numbers transcribed from M5GFX's own board bring-up
//! (`src/M5GFX.cpp`, the `board_M5PaperColor` branch of `autodetect`), which is
//! the working driver for this exact board.

use esp_hal::i2c::master::I2c;

/// M5PM1 I2C address (`m5pm1_i2c_addr` in M5GFX).
pub const ADDR: u8 = 0x6E;

const REG_I2C_CFG: u8 = 0x09;
const REG_WDT_CNT: u8 = 0x0A;
const REG_GPIO_OUT: u8 = 0x11;
const REG_GPIO_MODE: u8 = 0x10;
const REG_GPIO_PP: u8 = 0x13;
const REG_GPIO_FUNC: u8 = 0x16;

/// PMIC GPIO0 powers the e-paper panel; GPIO3 powers the SD card slot.
const EPD_AND_SD_BITS: u8 = (1 << 0) | (1 << 3);
/// Two function-select bits per pin, so pin N occupies bits `2N` and `2N+1`.
const EPD_AND_SD_FUNC_BITS: u8 = (0b11 << 0) | (0b11 << 6);

/// Bring up the panel's power rail. Returns an error if the PMIC does not ack.
pub fn enable_panel_power<'d, Dm: esp_hal::DriverMode>(
    i2c: &mut I2c<'d, Dm>,
    delay: &esp_hal::delay::Delay,
) -> Result<(), esp_hal::i2c::master::Error> {
    // read-modify-write helper, inline so it borrows `i2c` for the shortest time
    macro_rules! rmw {
        ($reg:expr, $data:expr, $mask:expr) => {{
            let mut buf = [0u8; 1];
            i2c.write_read(ADDR, &[$reg], &mut buf)?;
            let next = (buf[0] & $mask) | $data;
            i2c.write(ADDR, &[$reg, next])?;
        }};
    }

    // The PMIC's watchdog will cut power out from under us otherwise.
    rmw!(REG_WDT_CNT, 0x00, 0x00);

    // Claim GPIO0 (panel) and GPIO3 (SD) as plain GPIOs, drive them push-pull,
    // then raise them. Raising GPIO0 is the moment the panel gets power.
    rmw!(REG_GPIO_FUNC, 0x00, !EPD_AND_SD_FUNC_BITS);
    rmw!(REG_GPIO_MODE, EPD_AND_SD_BITS, 0xFF);
    rmw!(REG_GPIO_PP, 0x00, !EPD_AND_SD_BITS);
    rmw!(REG_GPIO_OUT, EPD_AND_SD_BITS, 0xFF);

    // Disable the PMIC's I2C idle-sleep. M5GFX notes that this register can be
    // left modified across a battery-powered reset, since the PMIC is always
    // powered and a shutdown does not reset it.
    rmw!(REG_I2C_CFG, 0x00, 0x00);

    delay.delay_millis(100);
    Ok(())
}

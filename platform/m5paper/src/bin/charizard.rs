//! **One species' picture out of the cartridge, held on the glass for ever.**
//!
//! The rest of this crate runs the emulator, and `src/main.rs` is honest about
//! what that is worth: twelve emulated frames a second and eight seconds a
//! repaint, so nobody plays a game on this. This is the other thing the panel
//! is for, and it is the thing it is actually good at — a still picture costs
//! nothing to *hold*. Pull the USB lead out and Charizard is still there.
//!
//! **It is a separate binary on purpose.** The brief that asked for it also
//! asked that the free-running gameplay and its A/B/C buttons keep working
//! exactly as they do; a second binary cannot interfere with them at all, and
//! it is the convention this crate already has for anything that is not the
//! emulator (`panel_probe`, `panel_deep_probe`, `panel_truncate`).
//!
//! ```sh
//! pixelgb species --rom ~/roms/blue.gb --out /tmp/species
//! TERMINALGB_SPRITE_PNG=/tmp/species/pokemon-blue/sprites/four-shade/1x/pokemon-front-006-charizard.png \
//!   tools/m5paper-build.sh charizard
//! ```
//!
//! # What is on the paper, exactly
//!
//! **The pixels are the cartridge's own and none of them were invented here.**
//! `pixelgb species` decompresses them from the player's own Pokémon Blue image
//! and they were checked byte for byte — 0 of 3,136 pixels differing — against
//! `pret/pokered`'s own committed `gfx/pokemon/front/charizard.png`, the
//! artist's original file that the compressed data in the cartridge was
//! compiled *from*. `docs/m5paper-charizard.md` has the run.
//!
//! **The colour is ours and Gen 1 never had any.** A DMG draws four greys and
//! this panel has six inks and no grey at all, so something has to give. Button
//! **C** cycles the same five looks the emulator's picture uses
//! (`gbscreen::LOOKS`), and the one this starts on is `red/yellow` — black
//! outline, red shadow, yellow light, white page. Its four inks are strictly
//! ordered by luminance (0, 57, 225, 255), so the artist's own light-to-dark
//! shading survives intact and only the hue is a choice. On a fire lizard that
//! choice happens to look right. **It is still a recolour, not "the real
//! colours", because there were none.** `dither` is the faithful alternative:
//! an ordered threshold that turns the four greys back into four apparent greys
//! on a panel that cannot make one.
//!
//! # Why the slow waveform
//!
//! `TRUE_COLOUR_PLL`, the vendor's own `0x08`, not the `0x07` the emulator
//! plays at. `src/main.rs` explains the trade: the fast value nearly halves a
//! repaint and the risk it carries is precisely that the inks stop separating.
//! Gameplay wants the seconds. A still picture is the one thing on this device
//! where fidelity is the entire point, so it pays the full 16 s once and then
//! costs nothing for ever.

#![no_std]
#![no_main]
#![allow(dead_code)]

#[path = "../font.rs"]
mod font;
#[path = "../gbscreen.rs"]
mod gbscreen;
#[path = "../geom.rs"]
mod geom;
#[path = "../panel.rs"]
mod panel;
#[path = "../pmic.rs"]
mod pmic;
#[path = "../sprite.rs"]
mod sprite;

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::main;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::{print, println};

use gbscreen::LOOKS;
use geom::{FrameBuffer, Rotation, FB_BYTES};
use panel::Ed2208;
use sprite::{Caption, Sprite};

esp_bootloader_esp_idf::esp_app_desc!();

/// **The picture, in flash.** `include_bytes!` puts it in `.rodata`, which the
/// ESP32-S3 memory-maps out of flash, so 792 bytes of sprite cost no RAM — the
/// same trick `src/main.rs` uses for the whole cartridge.
///
/// Written at build time by `src/sprite_host.rs` from a `pixelgb species` PNG;
/// **no game art is committed to this repository**, which is why this is an
/// environment variable and not a checked-in file.
static SPRITE: &[u8] = include_bytes!(env!("TERMINALGB_SPRITE"));

/// What the caption says. Not read from the blob: a sprite is pixels, and the
/// name and number are the build's own claim about which species it asked for.
/// `tools/m5paper-build.sh` fills them from the PNG's own file name so the two
/// cannot drift apart.
const NAME: &str = env!("TERMINALGB_SPRITE_NAME");
const NUMBER: &str = env!("TERMINALGB_SPRITE_NUMBER");

/// Landscape, as everywhere else in this crate.
const ORIENTATION: Rotation = Rotation::Landscape;

/// The vendor's own PLL. See the module docs: this is the one place on the
/// device where the slow waveform is the right answer.
const TRUE_COLOUR_PLL: u8 = 0x08;

/// Which look the first picture uses: `LOOKS[4]` is `red/yellow`.
const FIRST_LOOK: usize = 4;

static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

#[main]
fn main() -> ! {
    // 240 MHz. Nothing here is compute-bound, but the panel driver's own waits
    // are, and `Config::default()` does not pick the maximum.
    let p = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()),
    );
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor: a species picture, straight out of the cartridge ===");
    println!("sprite    : {} bytes in flash", SPRITE.len());

    let sprite = match Sprite::parse(SPRITE) {
        Ok(s) => {
            println!(
                "sprite    : {} #{} — {}x{}, four shades",
                NAME,
                NUMBER.trim_start_matches('#'),
                s.width(),
                s.height()
            );
            s
        }
        Err(e) => {
            // A panel with nothing on it and no reason given is the worst
            // outcome on a device with no screen of its own.
            println!("sprite    : REFUSED — {e}");
            println!("            rebuild with TERMINALGB_SPRITE_PNG set; see the module docs.");
            loop {
                delay.delay_millis(10_000);
            }
        }
    };

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
    let mut fb = FrameBuffer::new(buf, ORIENTATION);

    let mut buttons = [
        ("A", Flex::new(p.GPIO10)),
        ("B", Flex::new(p.GPIO9)),
        ("C", Flex::new(p.GPIO1)),
    ];
    for (_, pin) in buttons.iter_mut() {
        pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
        pin.set_input_enable(true);
    }

    let mut look_ix = FIRST_LOOK;
    println!();
    println!("buttons   : C cycles the look and repaints. A and B do nothing here.");
    println!("            the picture holds with the power off — that is the point.");
    println!();

    // The first picture, then one per press of C and no polling in between:
    // a still image is the whole feature, so the loop sleeps rather than spins.
    let mut was_c = false;
    loop {
        let look = LOOKS[look_ix];
        let ms = paint(&mut epd, &mut fb, &sprite, look);
        println!(
            "painted   : {NAME} {NUMBER}, look {}, PLL 0x{TRUE_COLOUR_PLL:02X}, {}.{:03} s",
            look.name(),
            ms / 1000,
            ms % 1000
        );
        println!("            press C for the next look.");

        // Wait for a fresh press of C. `was_c` is the level last seen, so a
        // held button is one press — the same edge rule `src/main.rs` uses, and
        // `GPIO1` has a hardware debounce capacitor that makes a short settle
        // read as still-held (see platform/m5paper/README.md).
        loop {
            let down = buttons[2].1.is_low();
            if down && !was_c {
                was_c = true;
                look_ix = (look_ix + 1) % LOOKS.len();
                break;
            }
            was_c = down;
            delay.delay_millis(20);
        }
    }
}

/// Compose the page and put it on the glass. Returns the refresh's wall time.
fn paint<Dm: esp_hal::DriverMode>(
    epd: &mut Ed2208<'_, Dm>,
    fb: &mut FrameBuffer<'_>,
    sprite: &Sprite<'_>,
    look: gbscreen::Look,
) -> u64 {
    let cap = Caption {
        name: NAME,
        number: NUMBER,
        lines: &["POKEMON BLUE", "FRONT SPRITE"],
    };
    if !sprite::draw(fb, sprite, look, &cap) {
        // The font reports a missing glyph rather than leaving a hole nobody
        // can see — the defect AGENTS.md records against the other 5x7 font.
        println!("caption   : a character has no glyph and was skipped");
    }
    epd.set_pll(TRUE_COLOUR_PLL);
    epd.send_framebuffer(fb);
    // AUTO lets the controller sequence its own power-up: worth about a second
    // and it does not touch the waveform (docs/m5paper-refresh.md).
    epd.refresh_auto(true, &mut || {}).total_ms
}

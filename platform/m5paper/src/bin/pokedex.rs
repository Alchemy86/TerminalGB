//! **Every species in Pokémon Blue, held on the glass one at a time.**
//!
//! `src/bin/charizard.rs` held exactly one picture, because that is what the
//! first ask was. This holds the whole Pokédex — all 151 front sprites, in
//! one flash archive (`src/pokedex.rs`) — and steps through it a button press
//! at a time. As before: a still picture is the one thing this panel is
//! actually good at, so it costs the full true-colour repaint once per press
//! and nothing at all in between.
//!
//! **It is a separate binary for the same reason `charizard.rs` was**: the
//! free-running gameplay in `src/main.rs` and its own A/B/C buttons must keep
//! working exactly as they do, and a second binary cannot interfere with them.
//!
//! ```sh
//! pixelgb species --rom ~/roms/blue.gb --out /tmp/species
//! TERMINALGB_SPECIES_DIR=/tmp/species/pokemon-blue/sprites/four-shade/1x \
//!   tools/m5paper-build.sh pokedex
//! ```
//!
//! # Buttons
//!
//! **C** advances to the next Pokédex entry and repaints. **A** goes back to
//! the previous one — the natural complement to C, and it costs nothing since
//! the button was otherwise idle. **B** cycles the same five colour looks
//! `charizard.rs` used to put on C (`gbscreen::LOOKS`); moving it here keeps
//! the picture's own advance button free for the picture, which is the thing
//! this firmware is actually for.
//!
//! # What is on the paper, exactly
//!
//! **The pixels are the cartridge's own.** `pixelgb species` decompresses all
//! 151 front sprites from the player's own Pokémon Blue image;
//! `src/pokedex_host.rs` is `src/sprite_host.rs`'s conversion run once per
//! species instead of once, and nothing here invents a pixel that was not
//! already in the archive. `docs/m5paper-pokedex.md` has the Charizard
//! verification this generalises — 0 of 3,136 pixels against `pret/pokered`'s
//! own committed source art.
//!
//! The colour, the recolour-not-real-colour caveat, and the slow vendor PLL
//! are all exactly `charizard.rs`'s reasoning, unchanged — see that history and
//! `docs/m5paper-pokedex.md` for it in full.

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
#[path = "../pokedex.rs"]
mod pokedex;

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
use pokedex::Archive;

esp_bootloader_esp_idf::esp_app_desc!();

/// **The whole Pokédex, in flash.** `include_bytes!` puts it in `.rodata`,
/// which the ESP32-S3 memory-maps out of flash — the ~108 KB of pictures for
/// 151 species costs no more RAM than the one 792-byte Charizard blob did,
/// which is the same trick `src/main.rs` uses for the whole cartridge.
///
/// Written at build time by `src/pokedex_host.rs` from a `pixelgb species`
/// directory; **no game art is committed to this repository**, which is why
/// this is an environment variable and not a checked-in file.
static ARCHIVE: &[u8] = include_bytes!(env!("TERMINALGB_POKEDEX"));

/// Landscape, as everywhere else in this crate.
const ORIENTATION: Rotation = Rotation::Landscape;

/// The vendor's own PLL. See `charizard.rs`'s history: this is the one place
/// on the device where the slow waveform is the right answer.
const TRUE_COLOUR_PLL: u8 = 0x08;

/// Which look the first picture uses: `LOOKS[4]` is `red/yellow`.
const FIRST_LOOK: usize = 4;

/// Where the browser opens: Charizard, #6, if the archive has one — matching
/// the flash it replaces rather than starting somewhere arbitrary. Falls back
/// to entry 0 if it does not, e.g. an archive built from a subset.
const START_NUMBER: u16 = 6;

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
    println!("=== M5PaperColor: the Pokédex, straight out of the cartridge ===");
    println!("archive   : {} bytes in flash", ARCHIVE.len());

    let archive = match Archive::parse(ARCHIVE) {
        Ok(a) => {
            println!("archive   : {} species, four shades each", a.len());
            a
        }
        Err(e) => {
            // A panel with nothing on it and no reason given is the worst
            // outcome on a device with no screen of its own.
            println!("archive   : REFUSED — {e}");
            println!("            rebuild with TERMINALGB_SPECIES_DIR set; see the module docs.");
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

    let mut current = archive.index_of(START_NUMBER).unwrap_or(0);
    let mut look_ix = FIRST_LOOK;
    println!();
    println!("buttons   : C next, A previous, B cycles the look.");
    println!("            the picture holds with the power off — that is the point.");
    println!();

    // The first picture, then one per press and no polling in between: a
    // still image is the whole feature, so the loop sleeps rather than spins.
    let mut was = [false; 3];
    loop {
        let entry = archive.entry(current).expect("current is always in range");
        let mut number_buf = [0u8; 4];
        let number = number_caption(entry.number, &mut number_buf);
        let look = LOOKS[look_ix];
        let ms = paint(&mut epd, &mut fb, &entry.sprite, entry.name, number, look);
        println!(
            "painted   : {} {number}, look {}, PLL 0x{TRUE_COLOUR_PLL:02X}, {}.{:03} s",
            entry.name,
            look.name(),
            ms / 1000,
            ms % 1000
        );
        println!("            {}/{} — C next, A previous, B cycles the look.", current + 1, archive.len());

        // Wait for a fresh press. `was[i]` is the level last seen for that
        // button, so a held button is one press — the same edge rule
        // `src/main.rs` uses, and `GPIO1` has a hardware debounce capacitor
        // that makes a short settle read as still-held (see
        // platform/m5paper/README.md).
        'wait: loop {
            for (i, (_, pin)) in buttons.iter_mut().enumerate() {
                let down = pin.is_low();
                if down && !was[i] {
                    was[i] = true;
                    match i {
                        0 => current = pokedex::prev_index(current, archive.len()),
                        1 => look_ix = (look_ix + 1) % LOOKS.len(),
                        _ => current = pokedex::next_index(current, archive.len()),
                    }
                    break 'wait;
                }
                was[i] = down;
            }
            delay.delay_millis(20);
        }
    }
}

/// `"#" + a 3-digit Pokédex number` into a caller-owned stack buffer. No
/// `alloc` in this firmware, so this replaces the `format!("#{n:03}")` a std
/// build would reach for.
fn number_caption(n: u16, buf: &mut [u8; 4]) -> &str {
    debug_assert!(n <= 999, "no Pokédex has a four-digit number");
    buf[0] = b'#';
    buf[1] = b'0' + (n / 100 % 10) as u8;
    buf[2] = b'0' + (n / 10 % 10) as u8;
    buf[3] = b'0' + (n % 10) as u8;
    core::str::from_utf8(buf).expect("all-ASCII by construction")
}

/// Compose the page and put it on the glass. Returns the refresh's wall time.
fn paint<Dm: esp_hal::DriverMode>(
    epd: &mut Ed2208<'_, Dm>,
    fb: &mut FrameBuffer<'_>,
    sprite: &sprite::Sprite<'_>,
    name: &str,
    number: &str,
    look: gbscreen::Look,
) -> u64 {
    let cap = sprite::Caption {
        name,
        number,
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

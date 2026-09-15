//! **TerminalGB's own emulator, running a cartridge on the M5Stack
//! PaperColor's E Ink panel.**
//!
//! The bring-up that came before this ([`docs/m5paper-first-boot.md`]) proved
//! the toolchain, the panel and the buttons and deliberately ran no emulator.
//! This runs one: the same `terminalgb` core the desktop and terminal
//! frontends use, built `#![no_std]` through its `embedded` cargo feature.
//!
//! Three things are worth knowing before reading further.
//!
//! **Nobody plays a game on this.** The panel takes 15.967 s to draw a picture
//! — measured, on this device, and it does not vary with the picture. Against a
//! 16.743 ms Game Boy frame that is 954x too slow. So the loop here is not a
//! frame loop: it emulates a burst of frames at full speed with nothing
//! attached to the screen, then spends sixteen seconds putting **one** of them
//! on the glass.
//!
//! **The cartridge is in flash, not in RAM.** `include_bytes!` puts it in
//! `.rodata`, which the ESP32-S3 memory-maps out of flash, and the `embedded`
//! feature makes the emulator's mappers hold a `&'static [u8]` rather than an
//! `Arc<[u8]>` — so a 1 MiB ROM costs no RAM at all and there is no
//! reference count in external PSRAM for `s32c1i` to trip over. The scout's
//! third named risk is therefore not dodged, it is absent.
//!
//! **Nothing here is allocated from PSRAM.** The panel framebuffer is a
//! 120,000-byte static in internal SRAM as it was at bring-up, and the heap
//! below is internal SRAM too.
//!
//! [`docs/m5paper-first-boot.md`]: ../../docs/m5paper-first-boot.md

#![no_std]
#![no_main]

extern crate alloc;

mod gbscreen;
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
use esp_hal::time::{Instant, Rate};
use esp_println::{print, println};

use gbscreen::{Look, LOOKS};
use geom::{FrameBuffer, Ink, Rotation, FB_BYTES};
use panel::Ed2208;

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

esp_bootloader_esp_idf::esp_app_desc!();

/// The cartridge, in flash.
///
/// **No ROM is committed to this repository and none ever will be.** The path
/// comes from the environment at build time:
///
/// ```sh
/// TERMINALGB_ROM=~/roms/blue.gb tools/m5paper-build.sh flash
/// ```
///
/// `tools/m5paper-build.sh` defaults it to the MIT dmg-acid2 image in
/// `third_party/`, which is the one cartridge this project may redistribute —
/// so a fresh clone builds and shows a picture with no game at all.
static ROM: &[u8] = include_bytes!(env!("TERMINALGB_ROM"));

/// The panel framebuffer, 120,000 bytes, allocated from PSRAM.
///
/// **Bring-up kept this in internal SRAM and this port cannot.** Constructing a
/// `Gameboy` needs about 317 KiB of stack — measured, from the stack guard's
/// own `sp` — because every constructor in the chain returns by value and the
/// temporaries pile into one frame. esp-hal gives the main stack whatever
/// internal DRAM `.data` and `.bss` leave, and this chip has about 335 KiB in
/// total, so 120,000 bytes of panel buffer plus 51,296 of machine left only
/// 154,824 for a 317,000-byte frame. Out here, internal SRAM is almost entirely
/// stack.
///
/// The hazard bring-up was avoiding is still absent rather than accepted: with
/// `--features embedded` the cartridge is a `&'static [u8]` in flash, not an
/// `Arc`, so no reference count lives in PSRAM for `s32c1i` to reach, and there
/// is exactly one task.
fn framebuffer() -> &'static mut [u8; FB_BYTES] {
    let flat: alloc::boxed::Box<[u8]> = alloc::vec![0u8; FB_BYTES].into_boxed_slice();
    let arr: alloc::boxed::Box<[u8; FB_BYTES]> = flat
        .try_into()
        .unwrap_or_else(|_| unreachable!("allocated with exactly this length"));
    alloc::boxed::Box::leak(arr)
}

/// The orientation the captain asked for, carried forward from bring-up.
const ORIENTATION: Rotation = Rotation::Landscape;

/// Emulated frames to run before the first picture.
///
/// Pokémon Blue spends about 900 frames on the copyright and Game Freak intro
/// before the title screen will even accept a button, so a first picture taken
/// much earlier than this is a logo. It costs a few seconds of emulation
/// against sixteen seconds of panel, so there is no reason to be mean with it.
const BOOT_FRAMES: u32 = 1_000;

/// Emulated frames per button press — about a second of game time.
const STEP_FRAMES: u32 = 60;

/// How long a button's press is held, in emulated frames. Gen 1's own menu code
/// delays several frames after accepting a press and does not see one that
/// arrives inside that delay, which is why this is not one frame.
const PRESS_FRAMES: u32 = 12;

/// A Game Boy frame, in microseconds: 70224 cycles at 4.194304 MHz.
const FRAME_US: u64 = 16_743;

#[main]
fn main() -> ! {
    // **240 MHz, explicitly.** `Config::default()` does not pick the maximum,
    // and on a chip this far from real time every multiple counts.
    let p = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()),
    );
    let delay = Delay::new();

    // **THE HEAP IS PSRAM AND THE STACK IS WHY.** esp-hal puts the main stack
    // between the end of `.bss` and a fixed top, so on this chip every byte of
    // internal-SRAM heap is a byte of stack taken — and the DRAM an app gets is
    // about 335 KiB in total. The three claims on it do not fit:
    //
    //   panel framebuffer   120,000  (a static, and it stays in internal SRAM)
    //   emulator heap      ~115,000  (the PPU's RGBA frame is 92,160 of it)
    //   construction stack ~150,000  (measured, below)
    //
    // Both failures were measured rather than predicted. At a 192 KiB internal
    // heap the stack came out at 10,352 bytes and `esp_hal::init`'s guard fired
    // before a word was printed; at 120 KiB it reached the cartridge header and
    // then took an exception inside a 92,160-byte `memcpy` — which is the PPU
    // framebuffer, to the byte.
    //
    // So the heap goes to external PSRAM and internal SRAM is left to `.bss`
    // and the stack. **The scout's PSRAM/atomics hazard is not dodged here, it
    // is absent**: with `--features embedded` the cartridge is a `&'static
    // [u8]` in flash rather than an `Arc`, so there is no reference count in
    // PSRAM for `s32c1i` to reach, and there is exactly one task.
    //
    // What stays in internal SRAM: the panel framebuffer, and the machine
    // itself (`build_machine`'s static) — which is where the Game Boy's own
    // VRAM, WRAM, HRAM and OAM live, so the emulator's hot path never leaves
    // fast memory. Only the PPU's output buffer, cartridge RAM and the APU's
    // resampler are out there, and none of those is touched per instruction.
    // `PsramConfig::default()` is `PsramMode::Auto`: octal first, then quad,
    // rather than this firmware asserting which chip the module carries.
    let psram =
        esp_hal::psram::Psram::new(p.PSRAM, esp_hal::psram::PsramConfig::default());
    let (psram_start, psram_len) = psram.raw_parts();
    // SAFETY: the region is what the PSRAM driver just mapped, it is added
    // once, and nothing else in this program allocates from it.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            psram_start,
            psram_len,
            esp_alloc::MemoryCapability::External.into(),
        ));
    }

    println!();
    println!("=== TerminalGB on the M5PaperColor ===");
    println!("chip      : ESP32-S3, Xtensa LX7");
    println!(
        "panel     : {}x{} native, {}x{} logical ({:?})",
        geom::PANEL_W,
        geom::PANEL_H,
        ORIENTATION.width(),
        ORIENTATION.height(),
        ORIENTATION
    );

    // --- 1. Panel power, behind the PMIC rather than a GPIO ------------------
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
            println!(
                "          the panel is unpowered; every SPI write below is a no-op."
            );
        }
    }

    // --- 2. Panel bus --------------------------------------------------------
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
    }

    let mut fb = FrameBuffer::new(framebuffer(), ORIENTATION);

    // --- 3. The cartridge ----------------------------------------------------
    println!();
    println!("--- cartridge ---");
    println!(
        "size      : {} KiB, in flash (.rodata) — no RAM copy",
        ROM.len() / 1024
    );
    print!("title     : ");
    for &b in &ROM[0x134..0x143] {
        if (0x20..0x7F).contains(&b) {
            print!("{}", b as char);
        }
    }
    println!();
    println!(
        "header    : mapper ${:02X}, rom ${:02X}, ram ${:02X}",
        ROM[0x147], ROM[0x148], ROM[0x149]
    );

    // `None` is the save path: there is no filesystem, and the `embedded`
    // feature makes that unrepresentable rather than merely unused.
    let gb: &mut Gameboy = match build_machine() {
        Ok(gb) => gb,
        Err(e) => {
            println!("REFUSED: {e}");
            println!("the cartridge image is not one this build can run. Halting.");
            loop {
                delay.delay_millis(1000);
            }
        }
    };

    // **A Game Boy has a sound chip.** With no player installed the emulator
    // has no APU at all rather than a quiet one — `NR52` reads back powered
    // off — and Gen 1 hangs on it: the Pokémon Center nurse never finishes, and
    // a whiteout never completes. See docs/measured/host.md. Nothing here can
    // hear it, so the samples are switched off; every register the cartridge
    // can read still behaves exactly as it would with a speaker attached.

    println!(
        "heap      : {} KiB used, {} KiB free of {} KiB PSRAM after building the machine",
        esp_alloc::HEAP.used() / 1024,
        esp_alloc::HEAP.free() / 1024,
        psram_len / 1024
    );

    // --- 4. Emulate, and say how fast ---------------------------------------
    println!();
    println!("--- emulating {BOOT_FRAMES} frames ---");
    let fps = run_frames(gb, BOOT_FRAMES);

    // --- 5. One frame on the glass ------------------------------------------
    let mut look_ix = 0usize;
    let mut frames_run = BOOT_FRAMES;
    println!();
    println!(
        "--- frame {frames_run} onto the panel ({}) ---",
        LOOKS[look_ix].name()
    );
    let ms = show(&mut epd, &mut fb, gb, LOOKS[look_ix]);
    println!("panel     : {}.{:03} s", ms / 1000, ms % 1000);
    println!();
    println!(
        "emulation is {}x faster than this panel can show it.",
        (ms * fps as u64) / 1000
    );

    // --- 6. Buttons ----------------------------------------------------------
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
    println!("  A (GPIO10) = Game Boy START, then {STEP_FRAMES} frames and a repaint");
    println!("  B (GPIO9)  = Game Boy A,     then {STEP_FRAMES} frames and a repaint");
    println!(
        "  C (GPIO1)  = next look ({} of them), same frame repainted",
        LOOKS.len()
    );
    println!(
        "each repaint is about sixteen seconds. that is the panel, not the software."
    );

    let mut was = [false; 3];
    loop {
        for i in 0..buttons.len() {
            let down = buttons[i].2.is_low();
            if down && !was[i] {
                println!();
                match i {
                    0 => {
                        println!("button A -> START, {STEP_FRAMES} frames");
                        tap(gb, KeypadKey::Start);
                        frames_run += STEP_FRAMES;
                        run_frames(gb, STEP_FRAMES);
                    }
                    1 => {
                        println!("button B -> A, {STEP_FRAMES} frames");
                        tap(gb, KeypadKey::A);
                        frames_run += STEP_FRAMES;
                        run_frames(gb, STEP_FRAMES);
                    }
                    _ => {
                        look_ix = (look_ix + 1) % LOOKS.len();
                        println!("button C -> look: {}", LOOKS[look_ix].name());
                    }
                }
                println!(
                    "frame {frames_run} onto the panel ({})...",
                    LOOKS[look_ix].name()
                );
                let ms = show(&mut epd, &mut fb, gb, LOOKS[look_ix]);
                println!("panel     : {}.{:03} s", ms / 1000, ms % 1000);
            }
            was[i] = down;
        }
        delay.delay_millis(20);
    }
}

/// Build the machine, out of line and into a `static`.
///
/// **`#[inline(never)]` and a `static` are both load bearing, and the reason is
/// the stack.** `Gameboy` is 51,296 bytes and every constructor in the chain
/// returns one by value; with fat LTO the whole chain inlines into its caller,
/// and each level's temporary lands in that one frame. Built inside `main` it
/// produced a **322,928-byte** prologue — measured, from the stack guard's own
/// `sp` against its `top` — which is larger than this chip's entire DRAM and
/// blew up before `esp_hal::init` could print a word. Out here the cost is
/// bounded to this function and `main` stays small.
///
/// The `static` is not an optimisation: it is where the machine lives for the
/// rest of the program, so the 51 KiB sits in `.bss` rather than being carried
/// on the stack for ever.
#[inline(never)]
fn build_machine() -> Result<&'static mut Gameboy, &'static str> {
    // `Model::Auto` is what a person with both consoles would do: the CGB flag
    // in the header picks the machine. `try_` because the bytes come from
    // outside and this build aborts on panic — a short or malformed image must
    // be refused with a message, not take the process down with it.
    let mut gb = Gameboy::try_new_with_model(ROM, None, terminalgb::mode::Model::Auto)?;
    println!("machine   : constructed");

    // **A Game Boy has a sound chip.** With no player installed the emulator
    // has no APU at all rather than a quiet one — `NR52` reads back powered
    // off — and Gen 1 hangs on exactly that: the Pokémon Center nurse never
    // finishes and a whiteout never completes (docs/measured/host.md). Nothing
    // here can hear it, so the *samples* are switched off; every register the
    // cartridge can read still behaves as it would with a speaker attached.
    gb.enable_audio(alloc::boxed::Box::new(terminalgb::sound::SilentPlayer));
    gb.set_render_audio(false);
    println!("machine   : apu installed");

    // **The machine lives on the PSRAM heap, and that is a measured
    // compromise rather than a preference.** Its VRAM, WRAM, HRAM and OAM are
    // inline in this struct and are touched several times per emulated
    // instruction, so internal SRAM would be far faster — measured at 3 fps out
    // here. It does not fit: constructing a `Gameboy` needs more than the
    // 268 KiB of stack that is left once 51 KiB of internal SRAM is reserved
    // for the machine, and it still did with `inline(never)` on every
    // constructor in the chain. See platform/m5paper/README.md.
    Ok(alloc::boxed::Box::leak(alloc::boxed::Box::new(gb)))
}

/// Run `n` frames as fast as the chip can, and report the rate.
///
/// The rate is the interesting number and nothing else on this device measures
/// it: the panel's own sixteen seconds say nothing about how fast the emulator
/// is, and the PSP anchor in `docs/portability.md` is an anchor, not a
/// measurement of this chip.
fn run_frames(gb: &mut Gameboy, n: u32) -> u32 {
    // Nothing reads the picture during a burst, and drawing it is over half of
    // an overworld frame (`docs/throughput.md`). Turning the pixels back on at
    // a frame boundary makes the *next* frame byte-identical to a machine that
    // never stopped drawing, so this costs the displayed picture nothing — it
    // is the same switch a corpus collector uses.
    gb.set_render_pixels(false);
    let start = Instant::now();
    for i in 0..n {
        gb.frame();
        // A silent minute and a hang look identical from outside, and this
        // chip is slow enough for that to matter.
        if i % 100 == 99 {
            println!(
                "            {}/{} frames, {} ms",
                i + 1,
                n,
                start.elapsed().as_millis()
            );
        }
    }
    let ms = start.elapsed().as_millis().max(1);
    let fps = (n as u64 * 1000 / ms) as u32;
    let realtime_pct = (n as u64 * FRAME_US / 1000) * 100 / ms;
    println!(
        "emulated  : {n} frames in {}.{:03} s = {fps} fps ({}% of a real Game Boy)",
        ms / 1000,
        ms % 1000,
        realtime_pct
    );
    fps
}

/// Hold a button for [`PRESS_FRAMES`] emulated frames, then release it.
fn tap(gb: &mut Gameboy, key: KeypadKey) {
    gb.keydown(key);
    for _ in 0..PRESS_FRAMES {
        gb.frame();
    }
    gb.keyup(key);
}

/// Put the emulator's current picture on the paper, and return how long the
/// panel took.
fn show<Dm: esp_hal::DriverMode>(
    epd: &mut Ed2208<'_, Dm>,
    fb: &mut FrameBuffer<'_>,
    gb: &mut Gameboy,
    look: Look,
) -> u64 {
    // One frame with the pixels back on. The emulator was fast-forwarded with
    // them off; this is the frame that reaches the paper, and it is exactly the
    // frame a machine that had been drawing all along would produce.
    gb.set_render_pixels(true);
    gb.frame();
    gbscreen::blit(fb, gb.image(), look);
    epd.send_framebuffer(fb);
    epd.refresh()
}

/// Referenced by the module docs so a reader can find the bring-up colours.
#[allow(dead_code)]
const _INK_STILL_EXISTS: Ink = Ink::White;

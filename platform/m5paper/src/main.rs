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
//! **Nobody plays a game on this.** The panel takes 7.725 s to draw a picture
//! — measured, on this device, and it does not vary with the picture. That is
//! already two settings faster than the vendor: its own sequence is 15.954 s,
//! and
//! [`docs/m5paper-refresh.md`] is how the difference was found and what else
//! was tried and did not work. Against a 16.743 ms Game Boy frame it is still
//! **461x too slow**. So the loop here is not a frame loop: it emulates a burst
//! of frames at full speed with nothing attached to the screen, then spends
//! eight seconds putting **one** of them on the glass.
//!
//! [`docs/m5paper-refresh.md`]: ../../../docs/m5paper-refresh.md
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

/// The orientation the maintainer asked for, carried forward from bring-up.
const ORIENTATION: Rotation = Rotation::Landscape;

/// Emulated frames to run before the first picture.
///
/// Pokémon Blue spends about 900 frames on the copyright and Game Freak intro
/// before the title screen will even accept a button, so a first picture taken
/// much earlier than this is a logo. It costs a few seconds of emulation
/// against eight seconds of panel, so there is no reason to be mean with it.
const BOOT_FRAMES: u32 = 1_000;

/// Emulated frames per button press — about a second of game time.
const STEP_FRAMES: u32 = 60;

/// How long a button's press is held, in emulated frames. Gen 1's own menu code
/// delays several frames after accepting a press and does not see one that
/// arrives inside that delay, which is why this is not one frame.
const PRESS_FRAMES: u32 = 12;

/// A Game Boy frame, in microseconds: 70224 cycles at 4.194304 MHz.
const FRAME_US: u64 = 16_743;

/// The PLL (`0x30`) a gameplay repaint runs at, against the vendor's `0x08`.
///
/// **This is the only thing found that changes how long a refresh takes**, and
/// it nearly halves it: 15.954 s at the vendor default against **8.741 s** at
/// `0x07`, measured by `src/bin/panel_probe.rs` on this device. Command `0x83`
/// (the addressing window) and the `DISPLAY_REFRESH` parameter byte were both
/// swept and both cost exactly the same at every value — see
/// [`docs/m5paper-refresh.md`] for the numbers and the reasoning.
///
/// **It does not make this playable and nothing does.** 8.741 s against a
/// 16.743 ms Game Boy frame is still 522x too slow. What it buys is a button
/// press that answers in nine seconds rather than sixteen.
///
/// **The picture at this value has not been checked by anyone on this
/// codebase.** A too-fast waveform loses colour separation rather than failing,
/// and `BUSY` rises just the same, so the timing above cannot see it and this
/// board has no MISO to read a register back through. The corroboration it does
/// have is Patrick Siegl's measured sweep of the same 4.0" panel
/// ([waveshareteam/e-Paper#387](https://github.com/waveshareteam/e-Paper/pull/387)),
/// which recommends `0x07` for this size having looked at it. If the ink comes
/// out muddy, this constant is the one thing to move — `0x06` is 10.003 s and
/// `0x05` is 11.127 s.
///
/// [`docs/m5paper-refresh.md`]: ../../../docs/m5paper-refresh.md
const PLAY_PLL: u8 = 0x07;

/// Whether a repaint hands the whole power-on/refresh/power-off sequence to
/// the controller with command `0x17` (AUTO) instead of driving it by hand.
///
/// **Worth a second, and it does not touch the waveform.** The hand-driven
/// sequence M5GFX uses spends ~1.5 s around the refresh on two 200 ms settles
/// and two `BUSY` handshakes; AUTO spends ~0.5 s. Measured interleaved, three
/// rounds each at `PLAY_PLL`: **8.737 s by hand against 7.725 s through AUTO**,
/// a saving of 1012 ms that did not vary by more than a millisecond. The
/// hand-driven rounds *between* the AUTO ones kept their own time exactly,
/// which is what says AUTO really does power the panel down at the end rather
/// than leaving it on.
///
/// Not in any vendor driver for this panel; it is in Patrick Siegl's list of
/// commands that respond on this glass and `panel_deep_probe` confirmed it
/// here. The booster values M5GFX rewrites before every refresh are replayed
/// through it, which costs nothing measurable and keeps that intent.
const USE_AUTO_SEQUENCE: bool = true;

/// The PLL the vendor init sets, and what the colour preview repaints at.
///
/// **Button C is a palette preview, so it must not be the fast path.** Its
/// whole job is to show what the six inks look like, and the risk a faster
/// waveform carries is precisely that the inks stop separating — so judging a
/// palette through it would be judging the wrong thing.
const TRUE_COLOUR_PLL: u8 = 0x08;

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
    let ms = show(&mut epd, &mut fb, gb, LOOKS[look_ix], PLAY_PLL, &mut || {});
    frames_run += 1; // the frame `show` ran to get the picture
    println!(
        "panel     : {}.{:03} s (PLL 0x{PLAY_PLL:02X} + AUTO; the vendor's own sequence is 15.954 s)",
        ms / 1000,
        ms % 1000
    );
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
    println!("  A (GPIO10) = Game Boy START");
    println!("  B (GPIO9)  = Game Boy A");
    println!(
        "  C (GPIO1)  = next look ({} of them), held frame, true colour",
        LOOKS.len()
    );
    println!("gameplay repaints run at PLL 0x{PLAY_PLL:02X} (~8 s); C repaints at the vendor's");
    println!("0x{TRUE_COLOUR_PLL:02X} (~15 s), because judging a palette through a faster waveform judges");
    println!("the wrong thing. if the inks only separate under C, 0x{PLAY_PLL:02X} is too fast and");
    println!("PLAY_PLL in src/main.rs is the constant to move (0x06 is ~10 s).");

    println!();
    println!("--- free-running ---");
    println!("the game runs on its own from here: {STEP_FRAMES} frames, a repaint, repeat.");
    println!("no button is needed and none is waited for.");

    free_run(&mut epd, &mut fb, gb, &mut buttons, &mut look_ix, &mut frames_run);
}

/// **The loop the maintainer asked for: the game runs, and the paper keeps up with
/// it as fast as paper can.**
///
/// What changed, and why it is not what was asked for word for word. The ask
/// was *"can we not just it continue to refresh the screen every second"*. One
/// second is not available and no amount of software makes it available: this
/// panel is measured at **7.7 s a repaint** at the fastest PLL that still
/// separates the inks, and `docs/m5paper-refresh.md` is the sweep that says so
/// — the waveform is the pigment moving, not a number anyone here can choose.
/// What *was* available is the half of the ask that was a defect rather than a
/// wish: the loop this replaces only emulated and only repainted on a button's
/// rising edge, so between presses the machine sat idle with a stale picture on
/// the glass. Now it never sits idle.
///
/// One turn of this loop is:
///
/// 1. spend any press that landed while the panel had the CPU,
/// 2. run [`STEP_FRAMES`] frames, sampling the buttons between each one,
/// 3. put the frame on the paper, sampling the buttons about once a
///    millisecond for the whole of the repaint.
///
/// **There is no delay anywhere in it.** The cadence is what the work costs —
/// the frame batch plus the panel — and adding anything on top would only make
/// the paper later than the paper has to be.
///
/// **A button is sampled in both of the places this loop spends time**, which
/// is the whole reason step 3 takes a closure. Measured on the device: a turn is
/// **13.027 s** — 4.933 s of frames and 7.726 s of panel — against a press of a
/// couple of hundred milliseconds, so a loop that only looked between the two
/// would miss nearly all of them. Sampled through both it is **60 samples in the
/// batch (~82 ms apart) and 7,510 through the repaint (~1.03 ms apart)**, and
/// the only window left unwatched is the **0.368 s** of SPI, blit and settling
/// inside [`show`], which has no wait loop to hang a poll on. See
/// `docs/m5paper-emulator.md`.
fn free_run<Dm: esp_hal::DriverMode>(
    epd: &mut Ed2208<'_, Dm>,
    fb: &mut FrameBuffer<'_>,
    gb: &mut Gameboy,
    buttons: &mut Buttons<'_>,
    look_ix: &mut usize,
    frames_run: &mut u32,
) -> ! {
    // `was` is the edge detector and `pending` is the latch it feeds. They are
    // separate because the press is almost never spent where it is seen: the
    // sample that catches it is usually inside the panel's own wait loop, which
    // is holding the bus and cannot repaint anything. So the press waits here
    // until the top of the next turn, which is what "its effect appears in
    // whichever repaint follows" means.
    let mut was = [false; 3];
    let mut pending = [false; 3];
    let mut cycle: u32 = 0;

    loop {
        cycle += 1;
        let turn = Instant::now();

        // **Nothing reads the picture until [`show`] asks for it.** Drawing is
        // over half of an overworld frame (`docs/throughput.md`) and `show`
        // turns the pixels back on for the one frame that reaches the paper —
        // which is byte-identical to a machine that never stopped drawing, so
        // this costs the picture nothing. The same switch a corpus collector
        // uses, and the same one `run_frames` uses for the boot burst.
        gb.set_render_pixels(false);

        // --- 1. Spend anything the panel's wait loop caught ------------------
        poll(buttons, &mut was, &mut pending);
        let look_press = core::mem::take(&mut pending[2]);
        if look_press {
            *look_ix = (*look_ix + 1) % LOOKS.len();
        }
        // A pad press on a preview turn stays latched rather than being spent
        // here: [`tap`] runs frames, and a frame that moved is not the still
        // the preview is for. It is spent on the turn after, which is the next
        // repaint either way.
        if !look_press {
            for (i, key) in [(0usize, KeypadKey::Start), (1, KeypadKey::A)] {
                if core::mem::take(&mut pending[i]) {
                    println!("button {} -> {:?}", buttons[i].0, key);
                    tap(gb, key);
                    *frames_run += PRESS_FRAMES;
                }
            }
        }

        // --- 2. Emulate, unless C is asking for a still ----------------------
        //
        // **A look preview holds the frame, and that is the same argument the
        // slow waveform is there for.** Button C exists to show what the six
        // inks look like; a frame that has also moved on since the last repaint
        // changes two things at once, and then the picture answers neither
        // question. So a C turn runs no frames: the previous turn's frame goes
        // back on the glass, slowly and in true colour, and the game carries on
        // from exactly where it was on the turn after.
        let emu = Instant::now();
        if !look_press {
            for _ in 0..STEP_FRAMES {
                gb.frame();
                poll(buttons, &mut was, &mut pending);
            }
            *frames_run += STEP_FRAMES;
        }
        let emu_ms = emu.elapsed().as_millis();

        // --- 3. Paper ---------------------------------------------------------
        let pll = if look_press { TRUE_COLOUR_PLL } else { PLAY_PLL };
        let panel_ms = show(epd, fb, gb, LOOKS[*look_ix], pll, &mut || {
            poll(buttons, &mut was, &mut pending)
        });
        // `show` runs one frame of its own to get the picture. Count it, or
        // the number below drifts a frame behind on every repaint, for ever.
        *frames_run += 1;

        let total = turn.elapsed().as_millis();
        println!(
            "cycle {cycle:>4}  frame {:>7}  emu {}.{:03} s  panel {}.{:03} s  turn {}.{:03} s  ({}, PLL 0x{pll:02X})",
            *frames_run,
            emu_ms / 1000,
            emu_ms % 1000,
            panel_ms / 1000,
            panel_ms % 1000,
            total / 1000,
            total % 1000,
            LOOKS[*look_ix].name()
        );
    }
}

/// The three buttons, as `main` builds them.
type Buttons<'d> = [(&'static str, u8, Flex<'d>); 3];

/// Sample the buttons once and latch any rising edge.
///
/// **Latching rather than acting is what makes this callable from inside the
/// panel driver.** It is run from three places — the top of a turn, between
/// emulated frames, and about once a millisecond through a repaint — and the
/// last of those is holding the SPI bus, so the only thing it may do is
/// remember. `was` is the level last seen and must persist across all three or
/// a held button reads as a new press on every sample.
fn poll(buttons: &mut Buttons<'_>, was: &mut [bool; 3], pending: &mut [bool; 3]) {
    for i in 0..buttons.len() {
        let down = buttons[i].2.is_low();
        if down && !was[i] {
            pending[i] = true;
        }
        was[i] = down;
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
/// it: the panel's own eight seconds say nothing about how fast the emulator
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
///
/// `during` is run about once a millisecond for the whole of the panel's wait
/// — see [`Ed2208::refresh_auto`]. It does not cover the SPI write of 120,000
/// bytes, the blit, or the settle either side of the waveform, which have no
/// wait loop to hang it on; those are the 0.368 s a turn spends unwatched.
fn show<Dm: esp_hal::DriverMode>(
    epd: &mut Ed2208<'_, Dm>,
    fb: &mut FrameBuffer<'_>,
    gb: &mut Gameboy,
    look: Look,
    pll: u8,
    during: &mut dyn FnMut(),
) -> u64 {
    // One frame with the pixels back on. The emulator was fast-forwarded with
    // them off; this is the frame that reaches the paper, and it is exactly the
    // frame a machine that had been drawing all along would produce.
    gb.set_render_pixels(true);
    gb.frame();
    gbscreen::blit(fb, gb.image(), look);
    epd.set_pll(pll);
    epd.send_framebuffer(fb);
    if USE_AUTO_SEQUENCE {
        epd.refresh_auto(true, during).total_ms
    } else {
        epd.refresh(during)
    }
}

/// Referenced by the module docs so a reader can find the bring-up colours.
#[allow(dead_code)]
const _INK_STILL_EXISTS: Ink = Ink::White;

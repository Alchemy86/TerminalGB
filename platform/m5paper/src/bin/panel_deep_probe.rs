//! **The experiments `panel_probe` would not do.**
//!
//! `panel_probe` sweeps registers whose meaning came out of a working driver:
//! an addressing window, a frame-rate divider, a parameter byte. This one
//! writes registers **nobody has documented for this controller**, and starts a
//! refresh it then cuts the power on. The maintainer authorised it explicitly on
//! his own device — *"All options are ok to test. It's my test board. Go nuts."*
//!
//! Kept as a second binary rather than a section of the first, because the two
//! deserve different trust. `panel_probe` is a repeatable instrument whose
//! numbers are in `docs/m5paper-refresh.md`; this is a one-off that can leave
//! the glass worse than it found it, and its results are guesses tested rather
//! than measurements taken.
//!
//! # What it tries, and the reasoning for each
//!
//! **1. `LUT_SEL` — the prize, and the one most likely to hurt.** The refresh is
//! long because a Spectra 6 waveform separates six pigments. A black-and-white
//! waveform would be a fraction of it — mono e-paper does a full update in
//! about 300 ms — and the only surface that could expose one is a
//! LUT-from-register bit. Two cousins whose opcodes overlap this family's put
//! `LUT_SEL` in **PSR's second byte, bit 7**: the SPD1656 (Solomon Systech) and
//! the JD79661. Ours is `0x5F, 0x69`, and `0x69` has bit 7 clear.
//!
//! The risk is precise rather than vague: if the bit means the same thing here
//! and no LUT has been written to the registers, the next refresh drives the
//! panel from whatever those registers happen to contain. E Ink's own ED2208
//! manual calls the part a *"Hardware TCON"*, which argues there is no register
//! LUT at all and the bit is inert — but that is an argument, not a datasheet,
//! and nobody has published one.
//!
//! **2. PSR byte 0 bit 4.** Patrick Siegl's reverse engineering of this exact
//! 4.0" panel identifies `D4` as `BWR` and found a single-byte `0x1B` works
//! where the vendor sends two. In the UC81xx family that bit picks
//! black/white/red against black/white — a *mode* bit, which is the shape of
//! thing that could shorten a waveform. Ours is `0x5F`, bit 4 set.
//!
//! **3. Temperature.** Many controllers pick a waveform by temperature, and
//! warmer means faster. `0x41` (TSE) is in Siegl's list of commands that
//! respond on this panel. Weak prior: seven minutes of back-to-back refreshes
//! in `panel_probe` never drifted by more than 3 ms, which is not what a
//! temperature-compensated waveform looks like on a self-heating panel.
//!
//! **4. `0x17` (AUTO).** Also in Siegl's list. Runs power-on, refresh and
//! power-off from one command. Cannot change the waveform; worth one refresh to
//! learn whether the command exists, because a shorter command sequence is free
//! if it does.
//!
//! **5. Truncation — no undocumented register at all, just less patience.**
//! Start the refresh and cut the power part-way. The pigments end up wherever
//! they had got to. **Whether part-way is legible is the only question here a
//! timer cannot answer**, and it is the one that would matter most: if two
//! seconds of waveform gives a readable black-and-white picture, that is a
//! further four times on top of the frame-rate win.
//!
//! # How it tries not to break anything
//!
//! **Every risky write is followed by a hardware reset, a full re-init and a
//! normal refresh at the vendor's settings.** That puts every register back to
//! the values M5GFX ships and re-settles the glass, so the exposure is one
//! refresh per experiment rather than a session. Each experiment runs **once**.
//!
//! **No drive voltage is raised.** `PWRR` (`0x01`), `BTST1`/`BTST2`/`BTST3`
//! (`0x05`/`0x06`/`0x08`) and `VDCS` (`0x82`) are not written. Over-voltage is
//! the one thing that damages e-paper outright and it cannot make a refresh
//! shorter, so it is the one category left alone whatever the authorisation —
//! the maintainer gave permission to test options, not to break the panel for no
//! possible gain.
//!
//! **It ends on a clean picture.** A full reset, a full re-init and the six-ink
//! test card at the vendor's own settings. Any ghosting, mottling or missing
//! ink on *that* is damage these experiments caused, and it is legible because
//! the final picture is not itself an experiment.
//!
//! Run it with `tools/m5paper-build.sh deepprobe`.

#![no_std]
#![no_main]
#![allow(dead_code)]

#[path = "../geom.rs"]
mod geom;
#[path = "../panel.rs"]
mod panel;
#[path = "../pmic.rs"]
mod pmic;

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::main;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::{print, println};

use geom::{FrameBuffer, Ink, Rotation, FB_BYTES};
use panel::{Ed2208, Refresh};

esp_bootloader_esp_idf::esp_app_desc!();

static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// Fractions of a 14.5 s stock waveform to cut the power at.
const TRUNCATIONS: [u64; 4] = [1_000, 2_000, 4_000, 7_000];

#[main]
fn main() -> ! {
    let p = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()),
    );
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor DEEP probe: undocumented registers ===");
    println!("every risky write is followed by reset + re-init + a stock refresh.");
    println!("no drive voltage is written. ends on a clean test card.");

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
        Err(e) => println!("FAILED: {:?} — every number below is a lie.", e),
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
    let mut fb = FrameBuffer::new(buf, Rotation::Landscape);

    // Baseline, so every experiment below has a same-session number to be
    // compared against rather than one from another run at another temperature.
    println!();
    println!("--- 0. baseline at the vendor's settings ---");
    fb.clear(Ink::White);
    epd.send_framebuffer(&fb);
    let base = epd.refresh_full();
    line("stock PSR 5F 69, PLL 08", base);

    // --- 1. LUT_SEL --------------------------------------------------------
    println!();
    println!("--- 1. PSR second byte bit 7 (LUT_SEL on two cousin controllers) ---");
    println!("    0x69 -> 0xE9. if a register LUT exists and is empty, this is");
    println!("    the refresh that finds out.");
    fb.clear(Ink::Black);
    epd.send_framebuffer(&fb);
    epd.raw(0x00, &[0x5F, 0xE9]);
    let r = epd.refresh_full();
    line("PSR 5F E9", r);
    verdict(&r, &base);
    recover(&mut epd, &mut fb, &mut 0);

    // --- 2. PSR byte 0 bit 4 -----------------------------------------------
    println!();
    println!("--- 2. PSR first byte bit 4 (Siegl calls it BWR) ---");
    fb.clear(Ink::White);
    epd.send_framebuffer(&fb);
    epd.raw(0x00, &[0x4F, 0x69]);
    let r = epd.refresh_full();
    line("PSR 4F 69", r);
    verdict(&r, &base);
    recover(&mut epd, &mut fb, &mut 0);

    // Siegl's own single-byte PSR, which works on his 4.0" where the vendor
    // sends two. If the second byte is simply ignored here, that is evidence
    // against LUT_SEL living in it whatever experiment 1 measured.
    println!();
    println!("--- 2b. PSR as one byte (Siegl's 0x1B) ---");
    fb.clear(Ink::Black);
    epd.send_framebuffer(&fb);
    epd.raw(0x00, &[0x1B]);
    let r = epd.refresh_full();
    line("PSR 1B", r);
    verdict(&r, &base);
    recover(&mut epd, &mut fb, &mut 0);

    // --- 3. Temperature ----------------------------------------------------
    println!();
    println!("--- 3. TSE (0x41): does a forced temperature pick the waveform? ---");
    for (label, v) in [("TSE 0x00", 0x00u8), ("TSE 0x87", 0x87), ("TSE 0x8F", 0x8F)] {
        fb.clear(Ink::White);
        epd.send_framebuffer(&fb);
        epd.raw(0x41, &[v]);
        let r = epd.refresh_full();
        line(label, r);
        fb.clear(Ink::Black);
        epd.send_framebuffer(&fb);
        let _ = epd.refresh_full();
    }
    recover(&mut epd, &mut fb, &mut 0);

    // --- 4. AUTO -----------------------------------------------------------
    // 0x17 with 0xA5 is power-on, refresh, power-off in one command on Siegl's
    // panel. It cannot change the waveform; this only asks whether it exists.
    println!();
    println!("--- 4. AUTO (0x17 0xA5): does the command exist here? ---");
    fb.clear(Ink::White);
    epd.send_framebuffer(&fb);
    let t = esp_hal::time::Instant::now();
    epd.raw(0x17, &[0xA5]);
    let mut went_low = false;
    let spin = esp_hal::time::Instant::now();
    while spin.elapsed().as_millis() < 200 {
        if epd.busy_is_low() {
            went_low = true;
            break;
        }
    }
    if went_low {
        epd.wait_until_idle(60_000);
        let ms = t.elapsed().as_millis();
        println!("  AUTO                          accepted, {}.{:03} s", ms / 1000, ms % 1000);
    } else {
        println!("  AUTO                          no response (BUSY never went low)");
    }
    recover(&mut epd, &mut fb, &mut 0);

    // --- 5. Truncation -----------------------------------------------------
    println!();
    println!("--- 5. cutting the power part-way through the waveform ---");
    println!("    the timings here are chosen, not measured; what matters is");
    println!("    whether the panel survives it and whether the picture is");
    println!("    legible, and only an eye can answer the second.");
    for ms in TRUNCATIONS {
        fb.clear(Ink::Black);
        epd.send_framebuffer(&fb);
        let r = epd.refresh_truncated(ms);
        print!("  cut at {:>5} ms                ", ms);
        if r.settled {
            println!("waveform had already finished");
        } else {
            println!("cut at {}.{:03} s, panel still responding", r.drf_ms / 1000, r.drf_ms % 1000);
        }
        recover(&mut epd, &mut fb, &mut 0);
    }

    // --- 6. Leave it clean -------------------------------------------------
    println!();
    println!("--- 6. reset, re-init, six-ink test card at the vendor's settings ---");
    epd.reset();
    epd.init();
    epd.set_pll(0x08);
    geom::draw_test_card(&mut fb);
    epd.send_framebuffer(&fb);
    let r = epd.refresh_full();
    line("test card, stock", r);

    println!();
    println!("LOOK AT THE PANEL. this last picture is NOT an experiment — it is a");
    println!("clean test card at the settings the vendor ships, drawn after a full");
    println!("reset. six ink bands, each boxed in black:");
    println!("  black, white, yellow, red, blue, green.");
    println!("compare against: tools/m5paper-build.sh testcard");
    println!("ghosting, mottling or a missing ink HERE is damage the experiments");
    println!("above caused. six clean bands means they cost the panel nothing.");
    println!();
    println!("deep probe complete. nothing further will be written to the panel.");
    loop {
        delay.delay_millis(1000);
    }
}

/// Reset, re-init, and a stock refresh — back to the vendor's every register.
fn recover<Dm: esp_hal::DriverMode>(
    epd: &mut Ed2208<'_, Dm>,
    fb: &mut FrameBuffer<'_>,
    _n: &mut i32,
) {
    epd.reset();
    epd.init();
    epd.set_pll(0x08);
    fb.clear(Ink::White);
    epd.send_framebuffer(fb);
    let _ = epd.refresh_full();
}

fn line(what: &str, r: Refresh) {
    print!("  {what:30}");
    if !r.saw_busy {
        println!("REFUSED (BUSY never went low; no refresh started)");
        return;
    }
    if !r.settled {
        println!("TIMED OUT after {}.{:03} s", r.drf_ms / 1000, r.drf_ms % 1000);
        return;
    }
    println!(
        "waveform {}.{:03} s   total {}.{:03} s",
        r.drf_ms / 1000,
        r.drf_ms % 1000,
        r.total_ms / 1000,
        r.total_ms % 1000
    );
}

/// Say whether a experiment moved the waveform, against the session's own
/// baseline rather than a number from another run.
fn verdict(r: &Refresh, base: &Refresh) {
    if !r.saw_busy || !r.settled {
        println!("      -> the controller did not complete a refresh.");
        return;
    }
    let d = r.drf_ms as i64 - base.drf_ms as i64;
    if d.abs() < 50 {
        println!("      -> no change ({d:+} ms). the bit is inert, or means something else.");
    } else {
        println!("      -> MOVED by {d:+} ms. worth following up.");
    }
}

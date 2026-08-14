//! The proof that the core stands alone.
//!
//! Build it with **no features at all** and it still runs a cartridge:
//!
//! ```text
//! cargo run --release --no-default-features --example headless -- game.gb 600
//! ```
//!
//! At that feature set the whole dependency tree is `terminalgb -> blip_buf`.
//! There is no window, no terminal, no audio device, no image codec and no
//! `$HOME` lookup anywhere on this path — which is the claim the README's
//! "Embedding the core" section makes, made checkable.
//!
//! It uses only the public embedding API:
//! [`Gameboy::new`], [`Gameboy::frame`], [`Gameboy::image`],
//! [`Gameboy::keydown`]/[`Gameboy::keyup`], [`Gameboy::rom_title`],
//! [`Gameboy::save_state`]/[`Gameboy::load_state`].

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

const WIDTH: usize = 160;
const HEIGHT: usize = 144;

/// FNV-1a over the framebuffer. Hand-rolled so the example needs no crate for
/// it — the same reason the test harness has its own.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Render the RGBA framebuffer as ASCII, one character per 4x8 block, so a
/// human can see that the picture is a picture. Terminal-free: this is just
/// `println!`, it works in a pipe, a log or a CI transcript.
fn ascii(frame: &[u8]) -> String {
    const RAMP: [u8; 5] = [b'#', b'+', b'-', b'.', b' '];
    let mut out = String::new();
    for y in (0..HEIGHT).step_by(8) {
        for x in (0..WIDTH).step_by(4) {
            let mut sum = 0u32;
            let mut n = 0u32;
            for dy in 0..8 {
                for dx in 0..4 {
                    let i = ((y + dy) * WIDTH + (x + dx)) * 4;
                    // Green channel is a fine luminance proxy for four greys.
                    sum += frame[i + 1] as u32;
                    n += 1;
                }
            }
            let lum = (sum / n.max(1)) as usize;
            out.push(RAMP[(lum * (RAMP.len() - 1)) / 255] as char);
        }
        out.push('\n');
    }
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: headless <rom.gb> [frames]");
            eprintln!("  runs the cartridge with no frontend and prints what it drew");
            std::process::exit(2);
        }
    };
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600);

    // The embedder reads the ROM. The core never opens a path it was not
    // given, and `None` here means "do not touch the filesystem for battery
    // saves either" — pass `Some(path)` to opt into `<rom>.sav`.
    let rom = match std::fs::read(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("headless: cannot read {path}: {e}");
            std::process::exit(1);
        }
    };

    let mut gb = Gameboy::new(rom, None);
    println!("rom            {}", gb.rom_title());
    println!("checksum       {:#06x}", gb.global_checksum());

    // Hold START for a moment part-way through: proves input reaches the
    // machine without any windowing system involved.
    for i in 0..frames {
        if i == frames / 2 {
            gb.keydown(KeypadKey::Start);
        }
        if i == frames / 2 + 6 {
            gb.keyup(KeypadKey::Start);
        }
        gb.frame();
    }

    let hash = fnv1a(gb.image());
    println!("frames         {frames}");
    println!("frame hash     {hash:#018x}");

    // Save-state round trip, still with no filesystem and no frontend: run a
    // clone forward from the snapshot and it must land on the same frame.
    let snapshot = gb.save_state();
    let mut restored = Gameboy::new(std::fs::read(&path).unwrap(), None);
    match restored.load_state(&snapshot) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("headless: save state did not load back: {e:?}");
            std::process::exit(1);
        }
    }
    for _ in 0..60 {
        gb.frame();
        restored.frame();
    }
    let (a, b) = (fnv1a(gb.image()), fnv1a(restored.image()));
    println!("state bytes    {}", snapshot.len());
    println!(
        "state re-run   {}",
        if a == b {
            "identical after 60 more frames"
        } else {
            "DIVERGED"
        }
    );

    println!();
    print!("{}", ascii(gb.image()));

    if a != b {
        std::process::exit(1);
    }
}

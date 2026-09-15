//! Turn one `pixelgb species` PNG into the blob `src/sprite.rs` reads.
//!
//! **This exists so that no game art is committed to this repository.** The
//! rule is the one `AGENTS.md` states for the whole project — facts and formats
//! are ours to learn, a cartridge's pixels are not ours to ship — and it is the
//! same rule `TERMINALGB_ROM` already follows for the cartridge itself. So the
//! picture is extracted from the player's own cartridge, by the player, and
//! reaches flash at *build* time:
//!
//! ```sh
//! pixelgb species --rom ~/roms/blue.gb --out /tmp/species
//! TERMINALGB_SPRITE_PNG=/tmp/species/pokemon-blue/sprites/four-shade/1x/pokemon-front-006-charizard.png \
//!   tools/m5paper-build.sh charizard
//! ```
//!
//! **The PNG decoder is the repository's own** (`testharness/pngdec.rs`,
//! dependency-free, written from RFC 1951 and the PNG specification for the
//! dmg-acid2 reference compare), and the shade quantiser is the firmware's own
//! (`gbscreen::shade_of`). Nothing here is a second implementation of anything.
//!
//! **Two conversions happen here and both invert a picture if they are wrong.**
//!
//! 1. **Alpha.** `pixelgb` writes a sprite's background as a flood-filled page
//!    of fully transparent pixels whose RGB is `(0, 0, 0)` — so anything that
//!    reads only the colour channels sees the page as *black*. It is composited
//!    onto white first, which is what `pixelgb`'s own reference test does when
//!    it calls a transparent pixel index 0. (`pngdec::Image::alpha` was added
//!    for this; it decoded colour type 6 and discarded the channel.)
//! 2. **Shade order.** `pret/pokered` and `pixelgb` number shades with **0 as
//!    white**; `gbscreen` and `sprite.rs` number them with **0 as black**,
//!    because a ramp's first ink is black. `shade_of` works in luminance and
//!    lands on the second convention, so the flip happens by using it rather
//!    than by a subtraction somebody could get backwards.

// `geom.rs` and `gbscreen.rs` carry plenty this converter does not use; it
// wants one function out of them and the whole panel driver comes with it.
#![allow(dead_code)]

#[path = "font.rs"]
mod font;
#[path = "geom.rs"]
mod geom;
#[path = "gbscreen.rs"]
mod gbscreen;
#[path = "../../../testharness/pngdec.rs"]
mod pngdec;
// Declared so `sprite.rs`'s own `use crate::{font, gbscreen, geom}` resolves:
// under a standalone `rustc` build this file is the crate root.
#[path = "sprite.rs"]
mod sprite;

/// Kept in step with `sprite.rs` by `sprite::MAGIC`; repeated rather than
/// imported because that file pulls `esp-hal`-free but crate-shaped modules in
/// and this is a standalone `rustc` build.
const MAGIC: &[u8; 4] = b"GBSP";

fn main() {
    let mut args = std::env::args().skip(1);
    let (src, dst) = match (args.next(), args.next()) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("usage: sprite_host <sprite.png> <out.bin> [preview-dir]");
            eprintln!();
            eprintln!("  <sprite.png>   a `pixelgb species` four-shade 1x PNG, e.g.");
            eprintln!("                 out/pokemon-blue/sprites/four-shade/1x/pokemon-front-006-charizard.png");
            eprintln!("  [preview-dir]  also render every look as a PPM, through the");
            eprintln!("                 firmware's own `sprite::draw` — the only way to see");
            eprintln!("                 the page before it costs sixteen seconds of panel");
            std::process::exit(2);
        }
    };
    let preview = args.next();

    let bytes = std::fs::read(&src).unwrap_or_else(|e| {
        eprintln!("cannot read {src}: {e}");
        std::process::exit(1);
    });
    let img = pngdec::decode(&bytes).unwrap_or_else(|e| {
        eprintln!("{src}: {e}");
        std::process::exit(1);
    });

    if img.width > u16::MAX as usize || img.height > u16::MAX as usize {
        eprintln!("{src}: {}x{} does not fit the header", img.width, img.height);
        std::process::exit(1);
    }

    // (1) the transparent page becomes white, then (2) luminance becomes a
    // shade index with 0 darkest.
    let flat = img.flatten_onto([255, 255, 255]);
    let shades: Vec<u8> = flat
        .chunks_exact(3)
        .map(|p| gbscreen::shade_of(p[0], p[1], p[2]) as u8)
        .collect();

    // A sprite that came out of a cartridge has four shades and a background.
    // Anything else means the wrong file was handed over — a colour render, a
    // thumbnail, a photograph — and drawing it would look like a bug in the
    // panel rather than in the command line.
    let mut present = [0usize; 4];
    for &s in &shades {
        present[s as usize] += 1;
    }
    if present.iter().filter(|&&n| n > 0).count() > 4 {
        unreachable!("shade_of returns 0..=3");
    }
    if present[3] == 0 {
        eprintln!("{src}: no page-coloured pixel at all — is this a sprite?");
        std::process::exit(1);
    }

    let mut out = Vec::with_capacity(8 + (shades.len() + 3) / 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(img.width as u16).to_le_bytes());
    out.extend_from_slice(&(img.height as u16).to_le_bytes());
    let mut packed = vec![0u8; (shades.len() + 3) / 4];
    for (i, &s) in shades.iter().enumerate() {
        packed[i >> 2] |= (s & 0b11) << (6 - 2 * (i & 3));
    }
    out.extend_from_slice(&packed);

    std::fs::write(&dst, &out).unwrap_or_else(|e| {
        eprintln!("cannot write {dst}: {e}");
        std::process::exit(1);
    });

    println!(
        "{src}\n  -> {dst}  {}x{}, {} bytes  (shades darkest-first: {:?})",
        img.width,
        img.height,
        out.len(),
        present
    );

    if let Some(dir) = preview {
        render_previews(&out, &dir);
    }
}

/// Render the composed page for every look, through the firmware's own
/// [`sprite::draw`], and decode it the way the panel does.
///
/// **This is how the page gets looked at at all.** Nobody on the development
/// end can see the e-paper — the whole reason `geom.rs` and `gbscreen.rs` are
/// host-testable — and `testcard_host.rs` already solves the same problem the
/// same way for the test card. A picture that is wrong here is wrong on the
/// glass, sixteen seconds and a flash cycle earlier.
fn render_previews(blob: &[u8], dir: &str) {
    use geom::{FrameBuffer, Ink, Rotation, FB_BYTES, PANEL_H, PANEL_W, ROW_BYTES};

    let sprite = sprite::Sprite::parse(blob).expect("we just wrote this blob");
    let name = std::env::var("TERMINALGB_SPRITE_NAME").unwrap_or_else(|_| "SPECIES".into());
    let number = std::env::var("TERMINALGB_SPRITE_NUMBER").unwrap_or_else(|_| "#000".into());
    let cap = sprite::Caption {
        name: &name,
        number: &number,
        lines: &["POKEMON BLUE", "FRONT SPRITE"],
    };

    std::fs::create_dir_all(dir).expect("preview dir");
    let mut buf = vec![0u8; FB_BYTES];
    for look in gbscreen::LOOKS {
        let slice: &mut [u8; FB_BYTES] = (&mut buf[..]).try_into().unwrap();
        let mut fb = FrameBuffer::new(slice, Rotation::Landscape);
        if !sprite::draw(&mut fb, &sprite, look, &cap) {
            eprintln!("preview: {} has a caption character with no glyph", look.name());
        }

        // Decode the 4bpp framebuffer the way the panel does, and present it
        // the way an eye sees it: landscape, so the portrait buffer is turned.
        let (vw, vh) = (Rotation::Landscape.width(), Rotation::Landscape.height());
        let mut ppm = format!("P6\n{vw} {vh}\n255\n").into_bytes();
        for y in 0..vh {
            for x in 0..vw {
                let (px, py) = Rotation::Landscape.to_panel_pub(x, y);
                let b = fb.raw()[py * ROW_BYTES + px / 2];
                let code = if px % 2 == 0 { b >> 4 } else { b & 0x0F };
                ppm.extend_from_slice(&rgb(code));
            }
        }
        let path = format!("{dir}/{}.ppm", look.name().replace('/', "-"));
        std::fs::write(&path, &ppm).expect("write preview");
        println!("  preview {path}  ({vw}x{vh}, as the eye sees it)");
    }
    let _ = (PANEL_W, PANEL_H, Ink::White);
}

/// The measured ink colours, as the panel's own firmware carries them
/// (`EPD_PALETTE_ORIGINAL`) and as M5GFX's `epd_palette` repeats them. Same
/// table as `testcard_host.rs`.
fn rgb(code: u8) -> [u8; 3] {
    use geom::Ink;
    match code {
        x if x == Ink::Black as u8 => [0, 0, 0],
        x if x == Ink::White as u8 => [255, 255, 255],
        x if x == Ink::Yellow as u8 => [255, 243, 56],
        x if x == Ink::Red as u8 => [191, 0, 0],
        x if x == Ink::Blue as u8 => [0, 0, 255],
        x if x == Ink::Green as u8 => [67, 138, 28],
        _ => [255, 0, 255], // a code the panel does not use: loud on purpose
    }
}

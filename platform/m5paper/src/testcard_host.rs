//! Renders the panel's expected picture on the host, from the same `geom.rs`
//! the device runs, and writes it as a PPM.
//!
//! Nobody on the development end can see the e-paper, so this is how the
//! orientation is checked against something other than a claim: render what the
//! firmware would put in the framebuffer, decode it the way the panel does, and
//! look at the picture. Build it with `tools/m5paper-build.sh --testcard`.

#[path = "geom.rs"]
mod geom;

use geom::{draw_test_card, FrameBuffer, Ink, Rotation, FB_BYTES, PANEL_H, PANEL_W, ROW_BYTES};
use std::io::Write;

/// The measured ink colours, as the panel's own firmware carries them
/// (`EPD_PALETTE_ORIGINAL`) and as M5GFX's `epd_palette` repeats them.
fn rgb(code: u8) -> [u8; 3] {
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

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "testcard.ppm".into());
    let rot = match args.next().as_deref() {
        Some("portrait") => Rotation::Portrait,
        Some("landscape-flipped") => Rotation::LandscapeFlipped,
        _ => Rotation::Landscape,
    };

    let mut buf = vec![0u8; FB_BYTES];
    let arr: &mut [u8; FB_BYTES] = (&mut buf[..]).try_into().unwrap();
    let mut fb = FrameBuffer::new(arr, rot);
    draw_test_card(&mut fb);

    // Two views. "panel" is the raw framebuffer as the controller scans it out,
    // which is portrait because the glass is. "view" is what an eye sees with
    // the device held in landscape, and is the one to compare against a photo.
    let as_view = std::env::var("M5PAPER_VIEW").is_ok();
    if as_view {
        let (vw, vh) = (rot.width(), rot.height());
        let mut px = Vec::with_capacity(vw * vh * 3);
        for y in 0..vh {
            for x in 0..vw {
                let (nx, ny) = rot.to_panel_pub(x, y);
                let b = fb.raw()[ny * ROW_BYTES + (nx >> 1)];
                let code = if nx & 1 == 0 { b >> 4 } else { b & 0x0F };
                px.extend_from_slice(&rgb(code));
            }
        }
        let mut f = std::fs::File::create(&out).expect("create output");
        write!(f, "P6\n{} {}\n255\n", vw, vh).unwrap();
        f.write_all(&px).unwrap();
        eprintln!("wrote {} ({}x{} as seen in {:?})", out, vw, vh, rot);
        return;
    }

    // Decode the 4bpp native buffer exactly as the controller scans it out:
    // row-major over the *panel's* 400x600, two pixels per byte, high nibble
    // first. If the rotation were wrong this image is where it shows.
    let mut px = Vec::with_capacity(PANEL_W * PANEL_H * 3);
    for y in 0..PANEL_H {
        for x in 0..PANEL_W {
            let b = fb.raw()[y * ROW_BYTES + (x >> 1)];
            let code = if x & 1 == 0 { b >> 4 } else { b & 0x0F };
            px.extend_from_slice(&rgb(code));
        }
    }

    let mut f = std::fs::File::create(&out).expect("create output");
    write!(f, "P6\n{} {}\n255\n", PANEL_W, PANEL_H).unwrap();
    f.write_all(&px).unwrap();
    eprintln!(
        "wrote {} ({}x{} panel-native, {:?} = {}x{} logical)",
        out,
        PANEL_W,
        PANEL_H,
        rot,
        rot.width(),
        rot.height()
    );
}

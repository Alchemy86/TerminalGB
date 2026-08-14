//! Write the camera's idle card to a PNG, at whatever size, so it can be
//! *looked at* rather than asserted about.
//!
//! ```text
//! cargo run --release --example idle_card -- 1280 720 target/idle-1280x720.png
//! ```
//!
//! It renders the card **through the real stream path**, so what lands in the
//! PNG is the frame a camera consumer receives — card letterboxed into the same
//! rectangle the game occupies, black bars and all. Drawing `card::draw_idle`
//! straight into a full-size buffer is not that: it was how this example used
//! to work, and it hid the fact that the card was full-bleed while the game was
//! letterboxed, so switching between them made the feed jump.
//!
//! Pass `--game` for the same geometry with a game frame in it instead. The two
//! PNGs must have the picture in exactly the same place.
use std::time::Instant;
use terminalgb::video::{
    Destination, Pace, PixelFormat, Source, VideoOut, VideoSpec, SRC_H, SRC_W,
};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let game = argv.iter().any(|a| a == "--game");
    let a: Vec<&String> = argv.iter().filter(|a| !a.starts_with("--")).collect();
    let w: u32 = a.first().and_then(|s| s.parse().ok()).unwrap_or(1280);
    let h: u32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(720);
    let out = a
        .get(2)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("target/idle-{w}x{h}.png"));

    let mut v = VideoOut::in_memory(VideoSpec {
        dest: Destination::Stdout,
        format: PixelFormat::Rgba,
        size: Some((w, h)),
        source: Source::Native,
        pace: Pace::Emulated,
        ..VideoSpec::new(Destination::Stdout)
    });
    let t = Instant::now();
    if game {
        // A checkerboard, so the picture's extent is unmistakable against the
        // bars and a wrong scale shows up as uneven squares.
        let mut f = vec![0u8; (SRC_W * SRC_H * 4) as usize];
        for y in 0..SRC_H {
            for x in 0..SRC_W {
                let on = ((x / 8) + (y / 8)) % 2 == 0;
                let c = if on { 0xC0 } else { 0x30 };
                let p = ((y * SRC_W + x) * 4) as usize;
                f[p..p + 4].copy_from_slice(&[c, c, c, 255]);
            }
        }
        v.frame(&f, SRC_W, SRC_H, t);
    } else {
        v.go_idle();
        v.idle(t);
    }

    let frame = &v.captured()[..(w * h * 4) as usize];
    if let Some(d) = std::path::Path::new(&out).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    image::RgbaImage::from_raw(w, h, frame.to_vec())
        .expect("buffer matches dimensions")
        .save(&out)
        .expect("write png");
    println!(
        "wrote {out} ({w}x{h}, {})",
        if game { "game frame" } else { "idle card" }
    );
}

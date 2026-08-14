//! Write the camera's idle card to a PNG, at whatever size, so it can be
//! *looked at* rather than asserted about.
//!
//! ```text
//! cargo run --release --example idle_card -- 640 480 target/idle-640x480.png
//! ```
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let w: u32 = a.first().and_then(|s| s.parse().ok()).unwrap_or(640);
    let h: u32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(480);
    let out = a
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("target/idle-{w}x{h}.png"));
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    gameboy::video::card::draw_idle(&mut buf, w, h);
    if let Some(d) = std::path::Path::new(&out).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    image::RgbaImage::from_raw(w, h, buf)
        .expect("buffer matches dimensions")
        .save(&out)
        .expect("write png");
    println!("wrote {out} ({w}x{h})");
}

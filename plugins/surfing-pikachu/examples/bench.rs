//! Where a frame's time goes: simulation, picture composition, or the
//! terminal. Prints microseconds per frame for the first two; the third is
//! whatever is left when the frontend misses its deadline.
use std::time::Instant;
use surfing_pikachu::game::{Game, Input};
use surfing_pikachu::render::FRAME_BYTES;
use surfing_pikachu::tiles::TileSet;

fn main() {
    let gfx = std::env::args().nth(1).map(std::path::PathBuf::from);
    let (tiles, _) = TileSet::load(gfx.as_deref());
    let mut g = Game::new(1234);
    let mut buf = vec![0u8; FRAME_BYTES];
    for _ in 0..600 {
        g.step(Input::default());
    }
    const N: u32 = 2000;
    let t = Instant::now();
    for _ in 0..N {
        g.step(Input::default());
    }
    let step = t.elapsed().as_secs_f64() / N as f64;
    let t = Instant::now();
    for _ in 0..N {
        g.render(&tiles, &mut buf);
    }
    let render = t.elapsed().as_secs_f64() / N as f64;
    println!(
        "step {:7.1} us   render {:7.1} us   sum {:7.1} us   (a frame is 16743 us)",
        step * 1e6,
        render * 1e6,
        (step + render) * 1e6
    );
}

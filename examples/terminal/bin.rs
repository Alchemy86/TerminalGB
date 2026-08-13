extern crate gameboy;

use gameboy::gameboy::{load_rom, Gameboy, RenderMode::Terminal};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: terminal <path-to-rom.gb>");
        std::process::exit(2);
    });

    match load_rom(&path) {
        Ok((data, filepath)) => {
            let gb = Gameboy::new(data, Some(filepath));
            gb.render(Terminal);
        }
        Err(e) => {
            eprintln!("error loading rom '{}': {:?}", path, e);
            std::process::exit(1);
        }
    }
}

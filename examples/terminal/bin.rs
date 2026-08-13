extern crate gameboy;

use gameboy::gameboy::run_terminal;
use std::path::PathBuf;

/// Terminal frontend.
///   `terminal <rom.gb>`  — launch straight into the game (no picker).
///   `terminal`           — open the ROM picker.
fn main() {
    let arg = std::env::args().nth(1);

    match arg.as_deref() {
        Some("-h") | Some("--help") => {
            eprintln!("usage: terminal [ROM.gb]");
            eprintln!("  with a ROM path: launch straight into the game");
            eprintln!("  with no args:    browse the ROM picker (rom_dir from config)");
        }
        Some(path) => run_terminal(Some(PathBuf::from(path))),
        None => run_terminal(None),
    }
}

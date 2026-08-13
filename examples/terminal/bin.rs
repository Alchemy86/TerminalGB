extern crate gameboy;

use gameboy::gameboy::run_terminal;
use std::path::PathBuf;

/// Terminal frontend.
///   `terminal <rom.gb>`  — launch straight into the game (no picker).
///   `terminal`           — open the ROM picker.
///
/// `--video-*` flags stream the picture to a file or FIFO for `ffmpeg`/OBS to
/// pick up (see `docs/video-out.md`). Not stdout here: the TUI owns stdout.
fn main() {
    // Video flags become `GB_VIDEO_*`, so an explicit flag beats the config
    // file and any inherited environment. Everything else is passed through.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = match gameboy::video::apply_cli_flags(args) {
        Ok(rest) => rest,
        Err(e) => {
            eprintln!("terminal: {e}");
            std::process::exit(2);
        }
    };

    match rest.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            eprintln!("usage: terminal [ROM.gb] [--video-out PATH ...]");
            eprintln!("  with a ROM path: launch straight into the game");
            eprintln!("  with no args:    browse the ROM picker (rom_dir from config)");
            eprintln!("video-out (see docs/video-out.md):");
            eprintln!("{}", gameboy::video::cli_usage());
        }
        Some(path) => run_terminal(Some(PathBuf::from(path))),
        None => run_terminal(None),
    }
}

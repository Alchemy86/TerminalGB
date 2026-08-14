extern crate terminalgb;

use terminalgb::gameboy::{load_rom, Gameboy, RenderMode::Desktop};

/// Desktop (OpenGL window) frontend.
///
/// This is the frontend that can stream to **stdout**, because nothing else
/// here writes there:
///
/// ```text
/// desktop game.gb --video-out - | ffmpeg -f rawvideo -pix_fmt rgba \
///     -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p out.mp4
/// ```
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = match terminalgb::video::apply_cli_flags(args) {
        Ok(rest) => rest,
        Err(e) => {
            eprintln!("desktop: {e}");
            std::process::exit(2);
        }
    };

    let path = rest.first().cloned().unwrap_or_else(|| {
        eprintln!("usage: desktop <path-to-rom.gb> [--video-out - ...]");
        eprintln!("video-out (see docs/video-out.md):");
        eprintln!("{}", terminalgb::video::cli_usage());
        std::process::exit(2);
    });

    match load_rom(&path) {
        Ok((data, filepath)) => {
            let gb = Gameboy::new(data, Some(filepath));
            gb.render(Desktop);
        }
        Err(e) => {
            eprintln!("error loading rom '{}': {:?}", path, e);
            std::process::exit(1);
        }
    }
}

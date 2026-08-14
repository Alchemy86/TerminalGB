extern crate terminalgb;

use terminalgb::gameboy::{load_rom, Gameboy};
use terminalgb::screen::desktop::DesktopOptions;

/// Desktop (OpenGL window) frontend.
///
/// This is the frontend that can stream to **stdout**, because nothing else
/// here writes there:
///
/// ```text
/// desktop game.gb --video-out - | ffmpeg -f rawvideo -pix_fmt rgba \
///     -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p out.mp4
/// ```
///
/// It is also the frontend an automated runner drives, which is where
/// `--model`, `--scale`, `--upscale` and `--speed` earn their place — see
/// `docs/shootout.md`. The combination a screenshot-comparing runner wants is
/// `--model dmg --scale 1 --upscale none --speed 15`.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = match terminalgb::video::apply_cli_flags(args) {
        Ok(rest) => rest,
        Err(e) => {
            eprintln!("desktop: {e}");
            std::process::exit(2);
        }
    };
    let (opts, rest) = match DesktopOptions::parse(rest) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("desktop: {e}");
            std::process::exit(2);
        }
    };

    let path = rest.first().cloned().unwrap_or_else(|| {
        eprintln!("usage: desktop <path-to-rom.gb> [options]");
        eprintln!("{}", DesktopOptions::usage());
        eprintln!("video-out (see docs/video-out.md):");
        eprintln!("{}", terminalgb::video::cli_usage());
        std::process::exit(2);
    });

    match load_rom(&path) {
        Ok((data, filepath)) => {
            // The console is decided when the machine is built, before a window
            // exists — `new_with_model` so an explicit `--model` beats
            // `$GB_MODEL` as well as the cartridge header.
            let gb = Gameboy::new_with_model(data, Some(filepath), opts.model);
            gb.render_desktop_with(opts);
        }
        Err(e) => {
            eprintln!("error loading rom '{}': {:?}", path, e);
            std::process::exit(1);
        }
    }
}

// The terminal frontend needs a terminal, and `screen::tui` is compiled out
// on `wasm32`. Cargo can feature-gate a `[[bin]]` but not target-gate one, so
// on wasm this binary is an empty shell rather than a build error: `cargo
// build --target wasm32-unknown-unknown` with default features must keep
// working (the same rule as `terminalgb-chiptune`'s bin).
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use terminalgb::gameboy::run_terminal;

/// Terminal frontend — the `terminalgb` binary (also built as the `terminal`
/// example, which `include!`s this file).
///   `terminalgb <rom.gb>`  — launch straight into the game (no picker).
///   `terminalgb`           — open the ROM picker.
///   `terminalgb music`     — open MUSIC MODE: the shelf of cartridges as albums.
///   `terminalgb login`     — sign in to the trade portal (device-code flow).
///   `terminalgb logout`    — forget the stored portal sign-in.
///
/// `--video-*` flags stream the picture to a file or FIFO for `ffmpeg`/OBS to
/// pick up (see `docs/video-out.md`). Not stdout here: the TUI owns stdout.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    // Video flags become `GB_VIDEO_*`, so an explicit flag beats the config
    // file and any inherited environment. Everything else is passed through.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = match terminalgb::video::apply_cli_flags(args) {
        Ok(rest) => rest,
        Err(e) => {
            eprintln!("terminalgb: {e}");
            std::process::exit(2);
        }
    };

    match rest.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            eprintln!("usage: terminalgb [ROM.gb | music | login | logout] [--video-out PATH ...]");
            eprintln!("  with a ROM path: launch straight into the game");
            eprintln!("  with no args:    browse the ROM picker (rom_dir from config)");
            eprintln!("  music:           MUSIC MODE — every ROM in rom_dir as an album, with");
            eprintln!("                   cover art from its own title screen, its music");
            eprintln!("                   playing, and `w` to save what you hear as a WAV.");
            eprintln!("                   See docs/music-mode.md.");
            eprintln!("video-out (see docs/video-out.md):");
            eprintln!("{}", terminalgb::video::cli_usage());
        }
        // The run command the captain asked for. `--music` too, because a
        // person who types one will type the other.
        Some("music") | Some("--music") | Some("-m") => run_music(),
        // The `gh auth login` shape: sign in once in a browser, then the
        // terminal is silent. Also reachable from inside the TUI with `@`.
        Some("login") | Some("--login") => std::process::exit(run_login()),
        Some("logout") | Some("--logout") => std::process::exit(run_logout()),
        Some(path) => run_terminal(Some(PathBuf::from(path))),
        None => run_terminal(None),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "portal"))]
fn run_login() -> i32 {
    let cfg = terminalgb::screen::config::Config::load();
    terminalgb::portal::login::run_cli(cfg.portal.as_deref())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "portal"))]
fn run_logout() -> i32 {
    terminalgb::portal::login::run_logout_cli()
}

/// Built without the `portal` feature: say how to get it rather than
/// pretending `login` is a ROM path that could not be found.
#[cfg(all(not(target_arch = "wasm32"), not(feature = "portal")))]
fn run_login() -> i32 {
    eprintln!(
        "terminalgb: the trade portal is not built into this binary — rebuild \
         with `--features portal`"
    );
    2
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "portal")))]
fn run_logout() -> i32 {
    run_login()
}

#[cfg(all(not(target_arch = "wasm32"), feature = "music"))]
fn run_music() {
    terminalgb::gameboy::run_music();
}

/// Built without the `music` feature: say how to get it, rather than silently
/// opening the picker as though `music` were a ROM that could not be found.
#[cfg(all(not(target_arch = "wasm32"), not(feature = "music")))]
fn run_music() {
    eprintln!(
        "terminalgb: music mode is not built into this binary — rebuild with \
         `--features music`"
    );
    std::process::exit(2);
}

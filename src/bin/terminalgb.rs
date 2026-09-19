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
///   `terminalgb --lobby`   — the World Trade Centre: a shared Cable Club room.
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
            eprintln!(
                "  music:           MUSIC MODE — every ROM in rom_dir as an album, with"
            );
            eprintln!(
                "                   cover art from its own title screen, its music"
            );
            eprintln!(
                "                   playing, and `w` to save what you hear as a WAV."
            );
            eprintln!("                   See docs/music-mode.md.");
            eprintln!(
                "  --lobby:         THE WORLD TRADE CENTRE — a shared Cable Club room."
            );
            eprintln!(
                "                   Run it in one window, then run it again in another:"
            );
            eprintln!(
                "                   the second finds the first by itself. Walk both to"
            );
            eprintln!(
                "                   the Cable Club and you meet. `--lobby-partner` boots"
            );
            eprintln!(
                "                   a driven console to meet instead of a person, and"
            );
            eprintln!(
                "                   `--lobby=HOST:PORT` joins a room by address. ROM from"
            );
            eprintln!(
                "                   the argument, else the most recently played. See"
            );
            eprintln!("                   docs/worldlobby.md.");
            eprintln!("video-out (see docs/video-out.md):");
            eprintln!("{}", terminalgb::video::cli_usage());
        }
        // The run command the maintainer asked for. `--music` too, because a
        // person who types one will type the other.
        Some("music") | Some("--music") | Some("-m") => run_music(),
        // The `gh auth login` shape: sign in once in a browser, then the
        // terminal is silent. Also reachable from inside the TUI with `@`.
        Some("login") | Some("--login") => std::process::exit(run_login()),
        Some("logout") | Some("--logout") => std::process::exit(run_logout()),
        // The World Trade Centre. Both spellings, because the launcher script
        // people actually type passes an unrecognised argument straight
        // through as the ROM path and either form arrives here intact.
        Some(a)
            if a == "lobby"
                || a == "--lobby"
                || a == "--lobby-alone"
                || a == "--lobby-partner"
                || a.starts_with("--lobby=") =>
        {
            std::process::exit(run_lobby(&rest))
        }
        Some(path) => run_terminal(Some(PathBuf::from(path))),
        None => run_terminal(None),
    }
}

/// `terminalgb --lobby` — the whole entry to the shared room.
///
/// Everything it needs beyond the flag comes from where every other launch
/// gets it: the cartridge is the argument if one was given, otherwise the most
/// recently played one; the save is whatever sits beside that cartridge; the
/// plugins are the configured ones (minus whatever would fight the real second
/// console for the cable, plus `gen1-lobby`). See `docs/worldlobby.md` §8.
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "plugin-gen1-lobby",
    feature = "tui"
))]
fn run_lobby(args: &[String]) -> i32 {
    use terminalgb::lobby::{PartnerOptions, SeatOptions};

    let flag = args[0].as_str();
    let join = flag.strip_prefix("--lobby=").map(str::to_string);
    // **The driven console is opt-in, and the reason is that a cable has two
    // ends.** Booting one by default fills the second seat before the person
    // opposite has run the command, and they then cannot reach the Trade Center
    // at all — the Cable Club receptionist wants a console on the wire, and the
    // only one there is already taken. `--lobby` twice is the headline case, so
    // that is what the bare flag does; `--lobby-partner` is the solo demo.
    // `--lobby-alone` is what that used to be called and still means it.
    let want_partner = flag == "--lobby-partner";

    // The ROM, the way the picker resolves one: an explicit path wins,
    // otherwise the top of the recent list. A person who has never played
    // anything is told so, rather than dropped into an empty room.
    let explicit = args[1..]
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from);
    let rom = match explicit.or_else(|| {
        terminalgb::screen::config::Recent::load()
            .paths
            .into_iter()
            .find(|p| p.is_file())
    }) {
        Some(r) => r,
        None => {
            eprintln!(
                "terminalgb: --lobby needs a cartridge, and the recent list is \
                 empty.\n  Play something once, or say `terminalgb --lobby \
                 /path/to/blue.gb`."
            );
            return 2;
        }
    };
    if !rom.is_file() {
        eprintln!("terminalgb: --lobby: cannot read {}", rom.display());
        return 2;
    }
    // **Absolute, before anything else sees it.** The driven partner is a child
    // process and the room's scratch directory is resolved against the working
    // directory, so a relative `blue.gb` typed here becomes a path that only
    // works from one directory — the same trap that spawning a terminal on a
    // relative binary path already cost this repository.
    let rom = rom.canonicalize().unwrap_or(rom);

    let name = std::env::var("GB_LOBBY_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "RED".to_string());
    // The partner needs a save that has *been* to a Cable Club, and the only
    // one to hand is the player's own — staged into `target/` by
    // `worldlobby play`, never opened where it lives. Without one it starts a
    // new game and never reaches the receptionist at all.
    let save = rom.with_extension("sav");
    let save = save.is_file().then_some(save);
    // Joining somebody else's room means the seats are already spoken for; the
    // partner is only for the person opening one on their own.
    let partner = (join.is_none() && want_partner).then(|| PartnerOptions {
        bin: None,
        name: "BLUE".to_string(),
        rom: rom.clone(),
        save: save.clone(),
        // Twelve hours. The driven partner stops when the frame budget runs
        // out, and nobody plays past that in one sitting.
        frames: 60 * 60 * 60 * 12,
        // Its output goes to a log file, not to the terminal, so the only cost
        // of asking for detail is disk — and without the stage transitions
        // "never reached the Trade Center" says nothing about where it stopped.
        verbose: std::env::var("GB_LOBBY_QUIET").is_err(),
    });

    println!("lobby: {}", rom.display());
    match terminalgb::lobby::take_seat(SeatOptions {
        rom,
        save: None,
        name,
        join,
        partner,
        stage_self: false,
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("terminalgb: lobby: {e}");
            1
        }
    }
}

/// Built without the room: say how to get it, rather than treating `--lobby`
/// as a cartridge that could not be found.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(all(feature = "plugin-gen1-lobby", feature = "tui"))
))]
fn run_lobby(_args: &[String]) -> i32 {
    eprintln!(
        "terminalgb: the World Trade Centre is not built into this binary — \
         rebuild with `--features tui,plugin-gen1-lobby`"
    );
    2
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

//! Launch configuration for the MCP server.
//!
//! # Why everything optional is off
//!
//! The normal client is a *model*, not a person: it wants the machine to run as
//! fast as the host can manage and it never looks at a window or listens to a
//! speaker. So the default is the leanest possible machine — no audio device,
//! no video sink, no plugins, no frame pacing — and every convenience is a flag
//! somebody had to type.
//!
//! Measured through the protocol itself (`docs/mcp.md` §6): headless with none
//! of them costs **0.238 ms/frame — 70x real time**. Attaching host audio costs
//! **0.264** — about 11%, *not* the ~8 ms/frame figure still in circulation,
//! which predates the audio crew's non-blocking hand-off (`docs/audio.md` §4c).
//! Audio is still off by default, but the honest reason is that nobody is
//! listening at 70x speed: the APU emits against *emulated* time, so at that
//! rate the queue is overrun and the oldest samples are discarded. Pair
//! `--audio` with `--speed realtime`.
//!
//! # What cannot be configured, and why
//!
//! There is no `--tui`. The MCP transport **is** stdin/stdout, and a terminal
//! frontend owns both — running them together would interleave escape
//! sequences with JSON-RPC frames and break the session. A human who wants to
//! watch gets [`Options::video_out`], which is a real stream into `ffmpeg`,
//! `mpv` or a virtual camera and costs +0.007 ms/frame at native size
//! (`docs/video-out.md`). Say the honest thing rather than shipping a flag that
//! corrupts the protocol.

use std::path::PathBuf;

use crate::mode::Model;

/// How fast the emulator is allowed to run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Speed {
    /// As fast as the host manages. The default: a model waits on the reply,
    /// so every millisecond spent sleeping is a millisecond of its budget.
    Uncapped,
    /// Pace to a multiple of the DMG's 59.7275 Hz, sleeping to make up the
    /// difference. `1.0` is real time — what you want when a human is watching
    /// [`Options::video_out`] or listening to `--audio`.
    Realtime(f64),
}

impl Speed {
    /// `uncapped`, `realtime`, `1x`, `2.5x`, `0.5`.
    pub fn parse(s: &str) -> Option<Speed> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "uncapped" | "max" | "none" => return Some(Speed::Uncapped),
            "realtime" | "real" | "normal" => return Some(Speed::Realtime(1.0)),
            _ => {}
        }
        let n: f64 = s.strip_suffix('x').unwrap_or(&s).parse().ok()?;
        if n.is_finite() && n > 0.0 {
            Some(Speed::Realtime(n))
        } else {
            None
        }
    }

    pub fn label(self) -> String {
        match self {
            Speed::Uncapped => "uncapped".to_string(),
            Speed::Realtime(n) if (n - 1.0).abs() < f64::EPSILON => {
                "realtime".to_string()
            }
            Speed::Realtime(n) => format!("{n}x"),
        }
    }
}

/// Everything a launch can decide.
#[derive(Debug, Clone)]
pub struct Options {
    /// Cartridge to load at startup. `None` leaves the server running with no
    /// machine, waiting for the `load_rom` tool — which is the right shape for
    /// a Claude Desktop entry that is not tied to one game.
    pub rom: Option<PathBuf>,
    /// Console. `Auto` reads the cartridge's CGB flag, as a person with both
    /// consoles would.
    pub model: Model,
    /// Give the cartridge its battery: load `<rom>.sav` at startup and write it
    /// back as the game saves. **On** by default, because a game that cannot
    /// save is not the game.
    ///
    /// `--no-battery` removes the battery entirely — **the existing save is not
    /// loaded either**, so the game starts as a fresh cartridge would. That is
    /// the honest behaviour of a cartridge with no battery, and it is worth
    /// stating plainly because "protect my save" is the obvious reason to reach
    /// for the flag and this does something stronger than that. To play an
    /// existing save without risking the file, copy the ROM **and its `.sav`**
    /// somewhere scratch and point the server there — the same rule the rest of
    /// this repo's harnesses follow.
    pub battery: bool,
    /// Where numbered save-state slots live. Defaults to
    /// `$XDG_STATE_HOME/terminalgb/states`, the same place the terminal frontend
    /// uses, so a state saved here loads there.
    pub state_dir: Option<PathBuf>,
    /// Attach a host audio device. Off by default: see the module note.
    pub audio: bool,
    /// Raw video-out destination — a file, a FIFO, `-` for stdout (refused
    /// here: stdout is the protocol), or a `/dev/video*` refusal with a recipe.
    pub video_out: Option<String>,
    /// Pixel format for the video stream.
    pub video_format: crate::video::PixelFormat,
    /// Plugins to attach by name, in order. Empty by default — **nothing is
    /// attached implicitly**, the same rule the other frontends follow.
    pub plugins: Vec<String>,
    pub speed: Speed,
    /// Hard ceiling on emulated frames any one tool call may run. A client that
    /// asks for a million frames gets `max_frames` and is told so, rather than
    /// the server disappearing for ten minutes.
    pub max_frames: u32,
    /// Expose the `write_memory` tool. **Off** by default and deliberately so:
    /// an agent that can poke RAM is not playing the game, it is editing it,
    /// and the resulting save is a corrupted one. On, it is a debugger.
    pub allow_write_memory: bool,
    /// NDJSON log of every request and response, for working out what a client
    /// actually asked for. Never stdout.
    pub log: Option<PathBuf>,
    /// Upscale factor applied to `screenshot` output. 1 keeps the image at
    /// 160x144, which is a few kilobytes of base64; 2 or 3 makes it legible in
    /// a chat transcript at four to nine times the tokens.
    pub screenshot_scale: u32,
    /// Whether the PPU generates pixels at all.
    ///
    /// **On by default, and the picture is the product** — this exists only so
    /// a caller whose observation comes out of memory can declare that it will
    /// not read the framebuffer and stop paying for one. Measured on a real
    /// cartridge from an overworld save state, that is **0.0927 → 0.0599
    /// ms/frame, a 1.55x**; in a battle, where the CPU has more to do, 1.25x.
    /// Guest-invisible by construction — see
    /// [`crate::gameboy::Gameboy::set_render_pixels`] — and the WRAM hash is
    /// identical either way, so nothing an observation reads can move.
    ///
    /// With it off, `screenshot` is **removed from `tools/list` and refused if
    /// called anyway**, exactly as `write_memory` is when it was not granted. A
    /// tool that answered with the last picture drawn before the flag took
    /// effect would be returning a stale frame as if it were the current one,
    /// which is worse than not offering it.
    pub pixels: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            rom: None,
            model: Model::Auto,
            battery: true,
            state_dir: None,
            audio: false,
            video_out: None,
            video_format: crate::video::PixelFormat::Rgba,
            plugins: Vec::new(),
            speed: Speed::Uncapped,
            max_frames: 60_000,
            allow_write_memory: false,
            log: None,
            screenshot_scale: 1,
            pixels: true,
        }
    }
}

/// The `--help` text. Kept here rather than in the binary so the flags and
/// their explanations cannot drift from the struct they set.
pub const USAGE: &str = "\
terminalgb-mcp — drive the TerminalGB Game Boy emulator from any MCP client

USAGE:
    terminalgb-mcp [OPTIONS]

The server speaks the Model Context Protocol over stdin/stdout. Point a client
at it; see docs/mcp.md for Claude Desktop and Claude Code config blocks.

CARTRIDGE
    --rom <PATH>          Load this cartridge at startup. Without it the server
                          starts with no machine and waits for the `load_rom`
                          tool.
    --model <M>           auto | dmg | cgb   (default: auto — read the CGB flag)
    --no-battery          Run the cartridge with NO battery: the existing .sav
                          is not loaded and none is written, so the game starts
                          fresh. To play an existing save without risking the
                          file, copy the ROM *and its .sav* somewhere scratch
                          and point --rom there instead.
    --state-dir <DIR>     Where save-state slots live
                          (default: $XDG_STATE_HOME/terminalgb/states)

SPEED  (the defaults are the fast ones)
    --speed <S>           uncapped | realtime | <N>x   (default: uncapped)
                          `uncapped` runs as fast as the host manages, which is
                          what a model wants. `realtime` paces to 59.7275 Hz,
                          which is what a human watching --video-out wants.
    --max-frames <N>      Ceiling on frames one tool call may run
                          (default: 60000 — about 17 minutes of game)

OUTPUT  (all off by default)
    --audio               Attach a host sound device. Needs the `audio` cargo
                          feature. Pair it with --speed realtime: the APU emits
                          against emulated time, so uncapped it overruns the
                          queue and the oldest samples are discarded.
    --video-out <PATH>    Stream raw frames to a file or FIFO so a human can
                          watch through ffmpeg/mpv. There is no --tui: stdio is
                          the protocol.
    --video-format <F>    rgba | rgb | yuyv   (default: rgba)
    --screenshot-scale <N>  Upscale the `screenshot` tool's PNG (default: 1)

PLUGINS  (none attached unless named)
    --plugins <A,B,...>   trainer | gen1-link | gen1-battle
                          Comma-separated, attached in the order given.

DEBUGGING
    --allow-write-memory  Expose the `write_memory` tool. Off by default: an
                          agent that pokes RAM is editing the game, not playing
                          it, and it will corrupt the save.
    --no-pixels           Declare that this client will not read the picture,
                          so the PPU may skip generating one. 1.25-1.55x faster
                          on a real cartridge, and guest-invisible: every byte
                          of memory an observation reads is unchanged. Removes
                          the `screenshot` tool, which would otherwise answer
                          with a stale frame.

    --log <PATH>          NDJSON log of every request and response.

    -h, --help            This text
    -V, --version         Version
";

/// Parse a command line. Returns the options, or a message to print and exit
/// with — `Ok(None)` means "printed something and there is nothing to run"
/// (`--help`, `--version`).
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Option<Options>, String> {
    let mut o = Options::default();
    let mut it = args.into_iter().peekable();
    // A value-taking flag with nothing after it is an error, never a silently
    // defaulted value: `--speed` with the argument lost to a shell quoting
    // mistake must not quietly run uncapped.
    macro_rules! value {
        ($flag:expr) => {
            it.next()
                .ok_or_else(|| format!("{} needs a value", $flag))?
        };
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("terminalgb-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--rom" => o.rom = Some(PathBuf::from(value!("--rom"))),
            "--model" => {
                let v = value!("--model");
                o.model = Model::parse(&v)
                    .ok_or_else(|| format!("unknown model '{v}' (auto|dmg|cgb)"))?;
            }
            "--no-battery" => o.battery = false,
            "--no-pixels" => o.pixels = false,
            "--state-dir" => o.state_dir = Some(PathBuf::from(value!("--state-dir"))),
            "--audio" => o.audio = true,
            "--video-out" => o.video_out = Some(value!("--video-out")),
            "--video-format" => {
                let v = value!("--video-format");
                o.video_format = crate::video::PixelFormat::parse(&v)
                    .ok_or_else(|| format!("unknown video format '{v}'"))?;
            }
            "--plugins" => {
                o.plugins = value!("--plugins")
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "--speed" => {
                let v = value!("--speed");
                o.speed = Speed::parse(&v).ok_or_else(|| {
                    format!("unknown speed '{v}' (uncapped|realtime|Nx)")
                })?;
            }
            "--max-frames" => {
                let v = value!("--max-frames");
                o.max_frames = v
                    .parse()
                    .map_err(|_| format!("--max-frames wants a number, got '{v}'"))?;
            }
            "--allow-write-memory" => o.allow_write_memory = true,
            "--log" => o.log = Some(PathBuf::from(value!("--log"))),
            "--screenshot-scale" => {
                let v = value!("--screenshot-scale");
                let n: u32 = v.parse().map_err(|_| {
                    format!("--screenshot-scale wants a number, got '{v}'")
                })?;
                o.screenshot_scale = n.clamp(1, 6);
            }
            other => {
                return Err(format!(
                    "unknown option '{other}' — run with --help for the list"
                ))
            }
        }
    }
    Ok(Some(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lean_configuration_is_the_default() {
        let o = parse(Vec::<String>::new()).unwrap().unwrap();
        assert!(!o.audio, "audio off by default — it costs ~8 ms/frame");
        assert!(o.video_out.is_none(), "no video sink by default");
        assert!(o.plugins.is_empty(), "nothing attached implicitly");
        assert_eq!(o.speed, Speed::Uncapped, "a model does not want pacing");
        assert!(!o.allow_write_memory, "poking RAM is opt-in");
    }

    #[test]
    fn speed_accepts_the_forms_a_person_would_type() {
        assert_eq!(Speed::parse("uncapped"), Some(Speed::Uncapped));
        assert_eq!(Speed::parse("MAX"), Some(Speed::Uncapped));
        assert_eq!(Speed::parse("realtime"), Some(Speed::Realtime(1.0)));
        assert_eq!(Speed::parse("2x"), Some(Speed::Realtime(2.0)));
        assert_eq!(Speed::parse("0.5"), Some(Speed::Realtime(0.5)));
        // Nonsense is refused rather than silently becoming a default.
        assert_eq!(Speed::parse("fast"), None);
        assert_eq!(Speed::parse("0x"), None);
        assert_eq!(Speed::parse("-1"), None);
    }

    /// A flag whose value the shell ate must be an error. Quietly running with
    /// the default is how a launch that says `--speed realtime` ends up
    /// uncapped and nobody finds out until the audio is unlistenable.
    #[test]
    fn a_flag_with_no_value_is_an_error() {
        let e = parse(vec!["--speed".to_string()]).unwrap_err();
        assert!(e.contains("--speed"), "{e}");
        let e = parse(vec!["--rom".to_string()]).unwrap_err();
        assert!(e.contains("--rom"), "{e}");
    }

    #[test]
    fn an_unknown_option_is_refused_with_a_pointer_to_help() {
        let e = parse(vec!["--tui".to_string()]).unwrap_err();
        assert!(e.contains("--tui") && e.contains("--help"), "{e}");
    }

    #[test]
    fn options_parse_together() {
        let o = parse(
            "--rom /tmp/a.gb --model dmg --speed realtime --plugins trainer,gen1-link \
             --audio --max-frames 100 --allow-write-memory"
                .split_whitespace()
                .map(String::from),
        )
        .unwrap()
        .unwrap();
        assert_eq!(o.rom.unwrap().to_str().unwrap(), "/tmp/a.gb");
        assert_eq!(o.model, Model::Dmg);
        assert_eq!(o.speed, Speed::Realtime(1.0));
        assert_eq!(o.plugins, vec!["trainer", "gen1-link"]);
        assert!(o.audio && o.allow_write_memory);
        assert_eq!(o.max_frames, 100);
    }
}

//! Terminal frontend — a proper TUI, not a bare image blitter.
//!
//! Two screens share one event loop:
//!   * **Picker** — browse/filter ROMs from the configured directory, with the
//!     cartridge's internal title, MBC type and recently-played first.
//!   * **Playing** — the game, drawn *centred* and scaled to fill the window,
//!     with framed status/controls panels, a help overlay and in-app ROM
//!     switching.
//!
//! Launching with a ROM path jumps straight into Playing (bypassing the
//! picker); launching with none opens the picker.
//!
//! Layout maths ([`fit_metrics`], [`popup_area`]), config parsing and ROM
//! header reading are all unit-tested without a terminal; the interactive parts
//! (rendering, input, protocols) are exercised by actually running it.
//
// This file was rebased on top of the dead-code/audio crew's hardening
// (commit 8357546); their intent is kept and mine layered on top:
//   * Audio: `init_audio` (feature-gated on `audio`, silent+graceful without a
//     device) is wired per-session in `load_session`, so sound follows the
//     active game across in-app ROM switches. The stream lives in `Session`.
//   * Protocol: their `build_picker` (prefer the advertised protocol, e.g.
//     Kitty over an env-guessed iTerm2; never panic on timeout) is used, then a
//     config `protocol` may override it.
//   * Loud failure: a terminal without the kitty keyboard protocol returns a
//     clear error before touching the screen (kept from their change).
//   * Scaling (mine wins — more complete): no `MAX_SCALE`; the image is
//     `Resize::Scale`-upscaled into a centred `fit_metrics` rect that grows
//     smoothly with the window. `Fit` can't upscale a 160x144 source, which is
//     what limited it to "small" and to a few coarse tiers.

use crate::gameboy::Gameboy;
use crate::input::KeypadKey;
#[cfg(feature = "plugin-agent")]
use crate::screen::agent_view as av;
use crate::screen::config::{
    CellScale, Config, LinkSetting, Palette, Recent, RenderPath, ShinyAlert, VolumeState,
};
use crate::screen::frame::FramePrep;
#[cfg(feature = "music")]
use crate::screen::music_view as mv;
use crate::screen::romlib::{self, RomEntry};
use crate::screen::termcaps;
use crate::screen::termcost;
#[cfg(feature = "plugin-trainer")]
use crate::screen::trainer_view as tv;
use crate::sound::output_volume;
use crate::telemetry::{self, AudioSample, Context, FrameSample, Telemetry, VideoSample};
use crate::termimage::{CellRect, TermImage, Wire};
#[cfg(feature = "plugin-trainer")]
use terminalgb_plugin_trainer::gen1::BattleKind;
#[cfg(feature = "plugin-trainer")]
use terminalgb_plugin_trainer::TrainerPlugin;

/// Stand-ins for the two panel modules when their plugin is not built in.
///
/// Only the handful of items the *always-compiled* layout code asks for, and
/// each answers "there is no such panel" — which is exactly true, and is also
/// what the layout would conclude anyway, since nothing can switch a panel on
/// when its plugin is absent.
#[cfg(not(feature = "plugin-trainer"))]
mod tv {
    /// Wider than any terminal, so the panel never fits — which is exactly
    /// true: without the plugin there is no panel. Not `u16::MAX`, only
    /// because comparing against a type's extreme is a clippy lint.
    pub const PANEL_MIN_W: u16 = 32_768;
}

#[cfg(not(feature = "plugin-agent"))]
mod av {
    use ratatui::layout::Rect;
    pub const PANEL_H: u16 = 0;
    /// All of the area is the game; there is no panel to split off.
    pub fn split(area: Rect, _rows: u16) -> (Rect, Option<Rect>) {
        (area, None)
    }
}
use crate::upscale::Upscaler;
use crate::video::{self, Source as VideoSource, VideoOut};

use std::env;
use std::error::Error;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ratatui::{
    backend::{Backend, CrosstermBackend},
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
            KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{
            disable_raw_mode, enable_raw_mode, EnterAlternateScreen,
            LeaveAlternateScreen, SetTitle,
        },
    },
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    Resize, StatefulImage,
};

use image::DynamicImage;

/// Append a line to the file named by `$GB_DEBUG_LAYOUT`, if set. Temporary
/// runtime instrumentation for diagnosing the scaling bug — writes to a file
/// (never the alternate screen). No-op with the env var unset, so it costs
/// nothing in normal use.
fn debug_layout(msg: &str) {
    if let Some(path) = env::var_os("GB_DEBUG_LAYOUT") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{msg}");
        }
    }
}

/// Wall-clock time one emulated frame occupies. The DMG refreshes at
/// 59.7275 Hz (4194304 Hz / 70224 cycles-per-frame), **not** a rounded 60 —
/// pacing at 16ms would run ~4.6% fast (62.5 fps), the same bug the audio crew
/// found on the desktop path. 1/59.7275 s ≈ 16743 µs. Emulation of a single
/// frame runs on the main loop; input is polled between frames.
const FRAME_TIME: Duration = Duration::from_micros(16_743);

/// Frames the linked second console is run *before* the pair starts stepping
/// together. See `App::open_link_partner` for why zero does not work.
const LINK_START_SKEW: u32 = 3;
/// The DMG refresh rate as a number, for "% of real hardware" reporting.
const DMG_HZ: f32 = 59.7275;
/// What this program calls its terminal window.
///
/// A **stable prefix**, deliberately. The camera carries the game picture and
/// nothing else (`docs/video-out.md`), so the documented way to stream the side
/// panel too is OBS window capture — and an OBS source finds its window by
/// title. The cartridge name is appended when one is loaded, because that is
/// genuinely useful in a taskbar, but the prefix never moves: an OBS Window
/// Capture set to *Match title, otherwise find window of same type* keeps
/// working across ROM switches, and on Wayland the portal picker makes the
/// title advisory anyway. See `docs/stream-panel-routes.md`.
const TERM_TITLE: &str = "TerminalGB";
/// Most emulated frames one loop iteration will run to catch up after a slow
/// draw. At the 16.743 ms frame period this absorbs a ~200 ms stall (a huge
/// terminal repaint, a window resize, the machine swapping) without the
/// emulator ever running *fast* afterwards; beyond it the deadline is resynced
/// and the extra frames are counted as dropped rather than fast-forwarded.
const MAX_CATCHUP: u32 = 12;
/// Input-poll timeout on the ROM picker, where nothing is due on a clock. Long
/// enough that an idle picker costs no measurable CPU, short enough that a
/// terminal resize is picked up immediately.
const IDLE_POLL: Duration = Duration::from_millis(50);
/// Width and height of the live stats panel, which gets a rectangle of its own
/// so it never lands on top of the game picture (see [`App::render_stats`]).
const STATS_W: u16 = 22;

/// Cells the game picture keeps whatever else is on screen. The trainer panel
/// shrinks below its design width rather than take these.
#[cfg_attr(not(feature = "plugin-trainer"), allow(dead_code))]
const MIN_GAME_W: u16 = 20;
const STATS_H: u16 = 15;
/// A held Game Boy key auto-releases this long after its last press/repeat.
/// In [`InputMode::Enhanced`] this is only a safety net — the kitty keyboard
/// protocol reports a real release, which normally arrives first — but it must
/// still exceed the terminal's key-autorepeat interval so held keys don't
/// flicker.
const KEY_HOLD: Duration = Duration::from_millis(110);

/// [`InputMode::Basic`] starting hold window, before the terminal's actual
/// auto-repeat rate has been observed (see [`Session::hold_window`]).
///
/// There is no key-release event in a plain terminal, so "still held" can only
/// be inferred from auto-repeat, and the window is a straight trade: too short
/// and a held direction flickers, too long and a tap keeps walking after you
/// let go. 160 ms covers auto-repeat rates down to ~6/s (X11's default is 30/s
/// = 33 ms) and costs at most ~9 emulated frames of overshoot — under one
/// walking step in a Gen 1 Pokémon game (16 frames).
const KEY_HOLD_BASIC: Duration = Duration::from_millis(160);
/// Bounds on the adaptive hold window. The floor keeps a fast auto-repeat from
/// shrinking it to nothing; the ceiling bounds how long a tap can over-run.
const KEY_HOLD_BASIC_MIN: Duration = Duration::from_millis(90);
const KEY_HOLD_BASIC_MAX: Duration = Duration::from_millis(350);
/// The largest gap between two presses of the same key that is still believed
/// to be auto-repeat *rate* rather than the auto-repeat *delay* (typically
/// 250-600 ms) or a deliberate double tap. Gaps above it teach us nothing.
const KEY_REPEAT_LEARN_MAX: Duration = Duration::from_millis(400);
/// In [`InputMode::Basic`], a one-shot control key (`f`, `[`, `]`, `+`, …)
/// arriving again within this window of the *previous* event for the same key
/// is the terminal auto-repeating, not the user pressing twice, and is ignored.
/// Sized above a typical auto-repeat interval and below a human double tap.
const CONTROL_REPEAT_LOCKOUT: Duration = Duration::from_millis(200);

/// How the terminal delivers key events to the game.
///
/// The kitty keyboard protocol is genuinely better — it reports key **release**,
/// so holding a direction is exact and two buttons can be down at once. Its
/// absence costs precisely that, and nothing else: see [`input_mode_for`] for
/// why we no longer refuse to start without it.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
enum InputMode {
    /// Kitty keyboard protocol: real press *and* release events.
    #[default]
    Enhanced,
    /// An ordinary terminal (GNOME Terminal, xterm, Konsole, tmux without
    /// passthrough…): presses only. Auto-repeat is the sole evidence that a key
    /// is still down, so a button is released by timeout.
    Basic,
}

impl InputMode {
    fn label(self) -> &'static str {
        match self {
            InputMode::Enhanced => "kitty · press+rel",
            InputMode::Basic => "basic · autorepeat",
        }
    }
}

/// Decide the input mode from what the terminal reported.
///
/// **This is the regression that made the emulator unusable in GNOME Terminal.**
/// `8357546` turned an `Ok(false)` here into `return Err(…)`, so the frontend
/// exited having drawn nothing. A missing input *nicety* may not cost the whole
/// program: every answer other than a clear "yes" degrades to
/// [`InputMode::Basic`], which is playable.
///
/// `Err` (query failed, timed out, not a tty) is treated exactly like
/// `Ok(false)` — an unanswered question is not a reason to refuse either.
fn input_mode_for(supported: Result<bool, std::io::Error>) -> InputMode {
    match supported {
        Ok(true) => InputMode::Enhanced,
        Ok(false) | Err(_) => InputMode::Basic,
    }
}

/// The one line the user gets when the kitty keyboard protocol is missing:
/// what is degraded, and how to get the better experience. Said once, in the
/// scrollback, in the picker footer, as a toast on entering a game and in the
/// `?` overlay — never every frame, and never as an error.
const BASIC_INPUT_NOTICE: &str =
    "input: this terminal does not report key *release* (no kitty keyboard \
     protocol), so a held button is let go shortly after the terminal stops \
     auto-repeating it. Everything is playable; holding a direction pauses once \
     before it walks smoothly, and two buttons at once only overlap briefly. \
     For exact press/release use Kitty, Ghostty, foot, or WezTerm with \
     `enable_kitty_keyboard = true`.";

/// Number of save-state slots the terminal exposes (`1`/`2`/`3`).
const NUM_SLOTS: u32 = 3;

/// Speed ladder. Each rung scales the wall-clock interval between emulated
/// frames relative to [`FRAME_TIME`] (`interval = FRAME_TIME * mult_num /
/// mult_den`); `1x` is real time. `MAX` is uncapped and instead runs a bounded
/// burst of frames per loop iteration so input/redraw stay responsive.
///
/// Audio interaction: the APU emits at a fixed sample rate, so running faster
/// than real time produces more samples than the host drains. At 1x the cpal
/// queue paces the emulator against the audio clock (high-watermark *wait*),
/// so every non-1x rung flips the player to drop-on-overflow via
/// [`Gameboy::set_audio_fast_forward`] — i.e. **fast-forward drops samples
/// rather than pitch-shifting**; slow-motion drains the queue, which holds the
/// last sample with a fade and re-primes (no hard silence steps). No
/// resampling, by choice. See docs/audio.md §4.
struct SpeedLevel {
    label: &'static str,
    mult_num: u32,
    mult_den: u32,
    burst: u32,
    uncapped: bool,
}

const SPEED_LADDER: &[SpeedLevel] = &[
    SpeedLevel {
        label: "0.5x",
        mult_num: 2,
        mult_den: 1,
        burst: 1,
        uncapped: false,
    },
    SpeedLevel {
        label: "1x",
        mult_num: 1,
        mult_den: 1,
        burst: 1,
        uncapped: false,
    },
    SpeedLevel {
        label: "2x",
        mult_num: 1,
        mult_den: 2,
        burst: 1,
        uncapped: false,
    },
    SpeedLevel {
        label: "4x",
        mult_num: 1,
        mult_den: 4,
        burst: 1,
        uncapped: false,
    },
    SpeedLevel {
        label: "MAX",
        mult_num: 0,
        mult_den: 1,
        burst: 8,
        uncapped: true,
    },
];
/// Index of the `1x` rung — the default.
const SPEED_NORMAL: usize = 1;

impl SpeedLevel {
    /// Wall-clock interval between frames at this speed (unused when `uncapped`).
    fn interval(&self) -> Duration {
        (FRAME_TIME * self.mult_num) / self.mult_den
    }
}

/// Where save states live. The core is filesystem-agnostic; the terminal decides
/// this. Slots are namespaced by cartridge title so games don't collide (and a
/// wrong-game load fails cleanly anyway via the state's ROM check). Prefers
/// `$XDG_STATE_HOME`, then `$HOME/.local/state`, else a temp dir.
fn state_path(title: &str, slot: u32) -> PathBuf {
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(env::temp_dir);
    // `config::app_dir_in` keeps the pre-rename `gameboy/` directory in use when
    // that is the one that exists, so the rename to TerminalGB cannot orphan
    // anybody's save states. See `config::LEGACY_APP_DIR`.
    let base = crate::screen::config::app_dir_in(&base).join("states");
    let _ = std::fs::create_dir_all(&base);
    let mut safe: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if safe.trim_matches('_').is_empty() {
        safe = "rom".to_string();
    }
    base.join(format!("{safe}.slot{slot}.gbss"))
}

/// Where recordings land when nothing is configured: the `video_dir` config
/// key, else `$XDG_VIDEOS_DIR/terminalgb`, else `~/Videos/terminalgb` (or the
/// pre-rename `gameboy` directory, when that is the one that exists).
fn video_dir(config: &Config) -> PathBuf {
    if let Some(d) = config.video_dir.as_ref() {
        return PathBuf::from(expand_home(d));
    }
    let base = env::var_os("XDG_VIDEOS_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join("Videos")))
        .unwrap_or_else(env::temp_dir);
    crate::screen::config::app_dir_in(&base)
}

/// Where screenshots land: the `screenshot_dir` config key, else
/// `$XDG_PICTURES_DIR/terminalgb`, else `~/Pictures/terminalgb` (or the
/// pre-rename `gameboy` directory, when that is the one that exists), else a
/// temp dir.
/// Somewhere a person will actually look — and the toast names the full path
/// anyway, because a screenshot that lands somewhere unknown is useless.
fn screenshot_dir(config: &Config) -> PathBuf {
    if let Some(d) = config.screenshot_dir.as_ref() {
        return PathBuf::from(expand_home(d));
    }
    let base = env::var_os("XDG_PICTURES_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")))
        .unwrap_or_else(env::temp_dir);
    crate::screen::config::app_dir_in(&base)
}

/// `~/x` → `$HOME/x`. The config file is hand-written, so a tilde is expected.
fn expand_home(s: &str) -> String {
    match s.strip_prefix("~/") {
        Some(rest) => match env::var_os("HOME") {
            Some(h) => format!("{}/{rest}", h.to_string_lossy()),
            None => s.to_string(),
        },
        None => s.to_string(),
    }
}

/// Cartridge title reduced to something safe in a filename. One definition,
/// in [`crate::video`], shared with the stream naming.
fn safe_title(title: &str) -> String {
    video::file_safe_title(title)
}

/// The last component of a path, for a status row that is ~18 columns wide.
///
/// A truncated *head* (`/home/user/Videos/game…`) tells the reader nothing;
/// the leaf is the part that distinguishes `gb.raw` from
/// `POKEMON_BLUE-…-160x144-rgba.raw`, which is exactly the distinction that was
/// missed. The full path is always in the `?` overlay's notices.
fn tail_of(dest: &str) -> String {
    Path::new(dest)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dest.to_string())
}

/// `YYYYMMDD-HHMMSS` for right now, UTC — see [`utc_stamp`].
fn now_stamp() -> String {
    utc_stamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
}

/// `YYYYMMDD-HHMMSS` in **UTC**, from a Unix timestamp.
///
/// UTC because turning it into local time needs the tz database, which means
/// `libc::localtime_r` and a platform story; a screenshot filename only has to
/// sort and be unique, and the suffix in [`unique_path`] guarantees the second.
/// Civil-from-days is Howard Hinnant's algorithm (public domain).
fn utc_stamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// The first free name of the form `stem.png`, `stem-2.png`, … so pressing the
/// key twice in the same second never overwrites the first shot.
fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let p = dir.join(format!("{stem}-{n}.png"));
        if !p.exists() {
            return p;
        }
    }
    first
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Which screen the frontend opens on.
#[derive(Clone, Debug, Default)]
pub enum Start {
    /// The ROM picker.
    #[default]
    Picker,
    /// Straight into a game.
    Rom(PathBuf),
    /// Music mode's shelf — see [`crate::music`].
    #[cfg(feature = "music")]
    Music,
}

/// Run the terminal frontend. `initial_rom` launches straight into that game;
/// `None` opens the ROM picker.
pub fn run(initial_rom: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    run_as(match initial_rom {
        Some(p) => Start::Rom(p),
        None => Start::Picker,
    })
}

/// [`run`], choosing which screen to open on.
pub fn run_as(start: Start) -> Result<(), Box<dyn Error>> {
    // The *only* legitimate refusal: no screen to draw on and no keyboard to
    // read. Anything short of that runs, degraded and honest. Checked before
    // the terminal is touched so the message lands in the shell, not in an
    // alternate screen we would then have to tear down.
    no_terminal_at_all()?;

    // Detect, adapt, inform — never refuse. The kitty keyboard protocol gives
    // real key up/down; without it we fall back to auto-repeat inference (see
    // `InputMode`) and say so once.
    let input =
        input_mode_for(ratatui::crossterm::terminal::supports_keyboard_enhancement());

    let (config, mut warnings) = Config::load_reporting();
    if input == InputMode::Basic {
        warnings.push(BASIC_INPUT_NOTICE.to_string());
    }
    let recent = Recent::load();

    // Video-out is resolved *here*, before the terminal is touched, so its
    // notices land in the scrollback with the config ones. It is not *opened*
    // here (or anywhere, until `v` is pressed) — see [`open_video`].
    let (video, video_template, video_autostart, warnings) =
        open_video(&config, warnings);

    // Say it before the alternate screen swallows it: a config key that does
    // not apply must never fail silently. These stay in the scrollback after
    // exit, and are repeated in the picker footer and the `?` help overlay.
    for w in &warnings {
        eprintln!("terminalgb: {w}");
    }

    // Terminal setup. Restore on panic so a crash doesn't leave the terminal
    // in raw mode / alternate screen.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic);
    }));

    enable_raw_mode()?;
    // Ask the terminal, once, whether it can read frames out of shared memory,
    // whether it answers a cursor report at all, and how long a bare round trip
    // costs. Must happen here: raw mode is on (so replies are not swallowed by
    // the line discipline) but the event loop has not started, so nothing else
    // is competing for stdin.
    probe_terminal();
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Name the window. This is not decoration: the supported way to get the
    // side panel into a stream is OBS window capture (docs/stream-panel-routes.md),
    // and an OBS source matches a window by its title. A terminal's default
    // title is the shell's idea of the command line, which changes on every
    // launch — `TERM_TITLE` is stable, so a scene set up once keeps working.
    // See `set_window_title` for why the prefix, not the whole string, is the
    // part to match on.
    let _ = execute!(stdout, SetTitle(TERM_TITLE));
    // Only ask for event-type reporting where the terminal said it understands
    // the protocol. Pushing the flags at a terminal that does not is not merely
    // useless — the matching pop on exit could leave stray bytes behind.
    if input == InputMode::Enhanced {
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        );
    }

    let backend = CrosstermBackend::new(CountingWriter(stdout));
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::with_warnings(
        config,
        recent,
        warnings,
        video,
        video_template,
        video_autostart,
    );
    app.input = input;
    match start {
        Start::Picker => {}
        Start::Rom(path) => app.start_game(&path), // a load error shows in the picker
        #[cfg(feature = "music")]
        Start::Music => app.enter_music(),
    }
    let res = app.run(&mut terminal);

    // Say goodbye on the wire before the terminal is restored. **Not a `Drop`
    // impl**: the release profile aborts on panic and this process can be
    // killed outright, so `Drop` is not a shutdown hook here — the same lesson
    // battery saves cost this project. Skipping it is survivable (the far end
    // sweeps a socket that no longer answers, and treats a peer that vanishes
    // while idle as an unplug rather than a crash), but a clean quit should
    // look like one, and it should not leave a socket behind.
    app.unplug_link();

    // Restore terminal.
    disable_raw_mode()?;
    if input == InputMode::Enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

/// The only condition that justifies refusing to start: nothing to draw on
/// **and** nothing to read keys from.
///
/// One of the two missing is survivable and is not our call to veto — output
/// redirected to a file still produces a video stream and telemetry, and input
/// redirected from a file is how a scripted run works. Both missing means there
/// is no terminal frontend to speak of, and saying so beats painting escape
/// sequences into a pipe.
fn no_terminal_at_all() -> Result<(), Box<dyn Error>> {
    if terminal_is_usable(io::stdout().is_terminal(), io::stdin().is_terminal()) {
        return Ok(());
    }
    Err(refusal_message().into())
}

/// The decision alone, so it can be tested without a terminal. Deliberately
/// permissive: **either** half is enough to run.
fn terminal_is_usable(stdout_tty: bool, stdin_tty: bool) -> bool {
    stdout_tty || stdin_tty
}

fn refusal_message() -> String {
    "neither stdout nor stdin is a terminal, so there is nothing to draw on and no \
     keys to read. Run this from a terminal, or use the headless routes: \
     `cargo run --release --example desktop -- <rom.gb>` for a window, or \
     `cargo run --release --example video_out -- <rom.gb>` to pipe raw frames."
        .to_string()
}

/// Resolve the configured raw video stream, if any, into the template `v` will
/// open — plus every notice worth showing the user, and whether the user asked
/// for it to be live from launch.
///
/// **Nothing is opened here, on purpose.** Naming a destination says *where*
/// frames go; it does not say *send them*. Conflating the two is what made a
/// terminal with a camera configured come up already streaming before its owner
/// had pressed anything — and, before that, made a FIFO destination park the
/// whole process in `open(2)` at startup with no interface and nothing to do but
/// `kill`. `v` is what goes live; `video_autostart = true` (or `--video-start`)
/// is the separate, explicit way to ask for it at launch.
///
/// Called from [`run`] **before** the terminal is put into the alternate screen,
/// so the `ffmpeg` command that consumes the stream — the single most useful
/// thing to leave in the scrollback — is not swallowed by it.
///
/// Stdout is refused for the same reason telemetry refuses it, only harder:
/// `ratatui` writes the whole UI to stdout, so a raw frame stream there would
/// destroy both. See `examples/video_out.rs` for the stdout-pipe producer.
fn open_video(
    config: &Config,
    mut warnings: Vec<String>,
) -> (VideoOut, Option<video::VideoSpec>, bool, Vec<String>) {
    let (spec, video_warnings) = video::VideoSpec::resolve(&config.video);
    warnings.extend(video_warnings);
    let (autostart, start_warnings) = video::resolve_autostart(&config.video);
    warnings.extend(start_warnings);
    let Some(mut spec) = spec else {
        return (VideoOut::disabled(), None, false, warnings);
    };
    if spec.dest == video::Destination::Stdout {
        warnings.push(
            "video: the terminal UI owns stdout — stream to a file or a FIFO \
             instead (mkfifo /tmp/gb.raw; video_out = \"/tmp/gb.raw\"), or use \
             `cargo run --release --example video_out` to pipe from a headless run"
                .to_string(),
        );
        return (VideoOut::disabled(), None, false, warnings);
    }
    // `source = display` with no explicit size: fix the geometry from the
    // configured upscaler now, so the size announced to the consumer is the
    // truth from the first byte. Cycling the upscaler mid-stream is then
    // letterboxed into it rather than breaking a running capture (see
    // `crate::video`).
    if spec.source == VideoSource::Display && spec.size.is_none() {
        let up = Upscaler::new(config.upscale, config.upscale_factor);
        let (w, h) = up.output_size(160, 144);
        spec.size = Some((w as u32, h as u32));
    }
    // Say where it *will* go, and how to turn it on. A destination configured
    // and then never mentioned again reads exactly like a setting that was
    // ignored, which is the other half of this confusion.
    warnings.push(format!(
        "{} ({})",
        spec.describe(None),
        if autostart {
            "video_autostart is on — live from launch"
        } else {
            "press `v` in-game to go live"
        }
    ));
    (VideoOut::disabled(), Some(spec), autostart, warnings)
}

/// What to say when a stream has just been started, including when it did not
/// come up live.
///
/// **A camera is never "waiting for a consumer".** A V4L2 output node takes
/// writes whether or not anybody is watching — that is the entire point of one
/// — so it is *not* waiting on a reader the way a FIFO genuinely is. When a
/// camera is not live the reason is always about the device (another producer
/// holds it, the module is not loaded, it is a real webcam), and the sink
/// already knows which. Saying "waiting for a consumer" there sent the captain
/// hunting for a consumer problem while the real one was a second emulator
/// still holding `/dev/video10` — the FIFO's words on the camera's screen.
fn waiting_toast(v: &VideoOut, short: &str) -> String {
    if v.state() != video::StreamState::Waiting {
        return format!("● {} → {short}", v.intent().word());
    }
    if v.is_camera() {
        return match v.device_error() {
            Some(why) => format!("⚠ camera {short} unavailable: {why}"),
            None => format!("⚠ camera {short} is not available yet"),
        };
    }
    format!("◌ waiting for a consumer on {short}")
}

/// Does this destination need the cartridge title before it can be opened?
fn needs_title(dest: &video::Destination) -> bool {
    match dest {
        video::Destination::Stdout => false,
        video::Destination::Path(p) => {
            let s = p.to_string_lossy();
            s.contains("%t") || s.contains("%s") || s.ends_with('/') || p.is_dir()
        }
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

enum Screen {
    Picker,
    Playing,
    /// Music mode: the same live machine, presented as a shelf of albums. See
    /// [`crate::music`] and [`crate::screen::music_view`].
    #[cfg(feature = "music")]
    Shelf,
}

/// Everything about the UI that can paint over a terminal image, as one
/// comparable value. See [`App::ui_signature`].
type UiSignature = (u8, bool, bool, bool, bool, bool, u8, bool);

#[derive(PartialEq, Eq)]
enum Overlay {
    None,
    Help,
    Quit,
}

/// The live game, present only while a ROM is loaded.
#[cfg_attr(not(feature = "plugin-trainer"), allow(dead_code))]
struct Session {
    gb: Gameboy,
    entry: RomEntry,
    paused: bool,
    /// The console on the other end of the link cable, when `link = <rom>` is
    /// configured. See [`crate::link`] and `docs/link-cable.md`.
    ///
    /// **`L` swaps which of the two `gb` is**, so everything downstream — the
    /// renderer, the palette, the upscaler, the screenshot keys, the plugins —
    /// keeps working on one machine and needs no idea a second exists. That is
    /// the whole reason the focused console is a *swap* rather than an index.
    /// Boxed: a whole `Gameboy` is large, and `Session` is built on the stack
    /// by every render test. Inlining a second one overflows it.
    link: Option<Box<LinkPartner>>,
    /// The automatically discovered cable to **another process**
    /// ([`crate::link::peer`]). `None` in the render tests and whenever
    /// `link = <rom>` put a second console in this process instead — the two
    /// are alternatives, because a Game Boy has one link port.
    peer: Option<Box<crate::link::peer::LinkPeer>>,
    /// Palette-mapping stage, owning its buffer so a frame allocates nothing.
    prep: FramePrep,
    /// Fast path: our own capped-resolution encoder. `None` when the protocol
    /// has no cell-addressed placement (sixel, halfblocks) or the user asked
    /// for [`RenderPath::Legacy`], in which case `image` below is used.
    term_image: Option<TermImage>,
    /// Legacy path: `ratatui-image` protocol state, rebuilt only when the frame
    /// actually changes (it used to be rebuilt every frame).
    image: Option<StatefulProtocol>,
    /// A **second** terminal image slot, for the trainer view's Pokémon sprite.
    /// Its own id, because Kitty images are addressed by id and reusing the
    /// game's would replace the picture (see `crate::termimage`).
    sprite_image: Option<TermImage>,
    /// Species currently encoded into `sprite_image`, so an unchanged party
    /// re-encodes nothing.
    sprite_key: Option<(u8, Palette)>,
    /// RGBA scratch for the sprite, reused so drawing it allocates nothing.
    sprite_rgba: Vec<u8>,
    sprite_dims: (u32, u32),
    /// The opposing trainer's roster, composed into ONE wide strip rather than
    /// six images. Six images means six ids, six placements and six PNGs a
    /// frame; one strip with transparent gutters looks identical and costs what
    /// a single sprite costs. See `compose_team_strip`.
    team_image: Option<TermImage>,
    /// Species + fainted flags + palette, so an unchanged roster is not
    /// recomposed.
    team_key: Option<(Vec<(u8, bool)>, Palette)>,
    team_rgba: Vec<u8>,
    team_dims: (u32, u32),
    /// Whether each panel picture was on screen last frame, so one that has
    /// stopped being drawn can be explicitly taken off the terminal.
    sprite_shown: bool,
    team_shown: bool,
    /// Hash of the last frame encoded on the legacy path.
    legacy_hash: u64,
    /// Last computed effective scale (for the status panel).
    scale: f32,
    /// Display rectangle and transmitted image size, for the status panel and
    /// telemetry — the gap between them is the whole optimisation.
    disp_cells: (u16, u16),
    /// Top-left cell of the picture, so nothing else can be laid out on top.
    disp_origin: (u16, u16),
    disp_px: (u32, u32),
    tx_px: (u32, u32),
    /// Escape-sequence bytes handed to the terminal for the last drawn frame.
    tx_bytes: usize,
    /// Bytes transmitted over the current one-second window, and the settled
    /// average from the last one.
    ///
    /// The status panel used to show the *last frame's* byte count, which reads
    /// `0B` on every deduplicated frame — on a static screen that is almost
    /// always, and even in motion it flickers between 0 and the real figure. A
    /// number that is zero half the time cannot tell anyone whether the
    /// transport is working, which is the one thing this panel is for.
    tx_window: usize,
    tx_per_frame: f32,
    /// Value of [`wrote_total`] at the end of the last draw, so the resize path
    /// can bill itself the bytes `ratatui` wrote for it.
    wrote_mark: u64,
    /// The image sequence to write straight to the terminal once the frame has
    /// been flushed. Empty when the picture did not change.
    pending_wire: String,

    // --- accounting -----------------------------------------------------
    /// Displayed frames per second, averaged over the last second.
    fps: f32,
    /// **Emulated** frames per second — the number that must stay at 59.7275
    /// whatever the display manages.
    emu_fps: f32,
    disp_count: u32,
    emu_count: u32,
    fps_mark: Instant,
    /// Cumulative totals for telemetry.
    emu_frames: u64,
    disp_frames: u64,
    /// Frames whose picture was byte-identical to the previous one, so nothing
    /// was encoded or transmitted.
    frames_identical: u64,
    /// Emulated frames that were never displayed because rendering was behind.
    frames_dropped: u64,
    /// Emulated frames since the last displayed one.
    skipped_since_display: u64,
    /// Display frames written to the terminal (as opposed to deduplicated away),
    /// cumulative. This is the "issued" half of the pair the status panel shows;
    /// see [`crate::screen::termcost`] for why there is no "presented" half.
    frames_issued: u64,
    /// Per-stage timings of the most recent displayed frame, milliseconds.
    t_emulate: f32,
    t_palette: f32,
    t_upscale: f32,
    t_encode: f32,
    t_write: f32,

    /// Per-button last press time *while held* (index by [`key_index`]);
    /// drives auto-release. `None` once the button has been released.
    held: [Option<Instant>; 8],
    /// Per-button last press time whether or not the button is still held.
    /// Kept separately from `held` on purpose: the gap we want to learn is
    /// between two auto-repeats, and on a terminal with a slow repeat rate the
    /// button has already auto-released by the time the next repeat lands — so
    /// learning from `held` alone would never converge.
    last_press: [Option<Instant>; 8],
    /// Shortest inter-repeat gap observed for this button, i.e. this terminal's
    /// auto-repeat *rate*. Only [`InputMode::Basic`] uses it; see
    /// [`Session::hold_window`].
    repeat_gap: [Option<Duration>; 8],
    /// Read-only handle on the host audio queue, for telemetry. `sound.rs` is
    /// not touched: [`crate::sound::cpal_audio::CpalPlayer::queue_handle`] is
    /// the diagnostic handle it already publishes.
    #[cfg(feature = "audio")]
    audio_queue:
        Option<std::sync::Arc<std::sync::Mutex<crate::sound::sample_queue::SampleQueue>>>,
    #[cfg(feature = "audio")]
    audio_rate: u32,
    /// Host-audio stream for this game. Must stay alive for the whole session —
    /// dropping it stops the sound — so it is dropped when the session ends or a
    /// different ROM is loaded. `()` when built without the `audio` feature.
    _audio: AudioStream,
}

/// The second console, and the wire joining it to the first.
struct LinkPartner {
    gb: Gameboy,
    cable: crate::link::LinkCable,
    /// Title of the cartridge in `gb` — i.e. the console that is **not** on
    /// screen. Kept rather than read per frame because `rom_title` builds a
    /// `String` out of the header every call, and this only changes on `W`.
    title: String,
}

impl Session {
    /// Assemble a session from a loaded machine and its terminal-image slots.
    ///
    /// One constructor because there are two callers — the game screen and
    /// music mode — and a second copy of forty field initialisers is how the
    /// two quietly stop agreeing about what a fresh session is.
    #[allow(clippy::too_many_arguments)]
    fn new(
        gb: Gameboy,
        entry: RomEntry,
        link: Option<Box<LinkPartner>>,
        peer: Option<Box<crate::link::peer::LinkPeer>>,
        term_image: Option<TermImage>,
        sprite_image: Option<TermImage>,
        team_image: Option<TermImage>,
        #[cfg(feature = "audio")] audio_queue: Option<
            std::sync::Arc<std::sync::Mutex<crate::sound::sample_queue::SampleQueue>>,
        >,
        #[cfg(feature = "audio")] audio_rate: u32,
        audio: AudioStream,
    ) -> Session {
        Session {
            gb,
            entry,
            paused: false,
            link,
            peer,
            prep: FramePrep::new(),
            term_image,
            image: None,
            sprite_image,
            sprite_key: None,
            sprite_rgba: Vec::new(),
            sprite_dims: (0, 0),
            team_image,
            team_key: None,
            team_rgba: Vec::new(),
            team_dims: (0, 0),
            sprite_shown: false,
            team_shown: false,
            legacy_hash: 0,
            scale: 1.0,
            disp_cells: (0, 0),
            disp_origin: (0, 0),
            disp_px: (0, 0),
            tx_px: (160, 144),
            tx_bytes: 0,
            tx_window: 0,
            tx_per_frame: 0.0,
            wrote_mark: wrote_total(),
            pending_wire: String::new(),
            fps: 0.0,
            emu_fps: 0.0,
            disp_count: 0,
            emu_count: 0,
            fps_mark: Instant::now(),
            emu_frames: 0,
            disp_frames: 0,
            frames_identical: 0,
            frames_dropped: 0,
            skipped_since_display: 0,
            frames_issued: 0,
            t_emulate: 0.0,
            t_palette: 0.0,
            t_upscale: 0.0,
            t_encode: 0.0,
            t_write: 0.0,
            held: [None; 8],
            last_press: [None; 8],
            repeat_gap: [None; 8],
            #[cfg(feature = "audio")]
            audio_queue,
            #[cfg(feature = "audio")]
            audio_rate,
            _audio: audio,
        }
    }

    /// How long button `idx` stays down after its last press.
    ///
    /// Under the kitty keyboard protocol this is a fixed safety net — a real
    /// release event normally lands first. Without it there is no release
    /// event, so the window *is* the release, and it adapts to the terminal:
    /// three observed auto-repeat intervals, bounded. Three because a single
    /// dropped repeat (a busy frame, a slow pty) must not read as "let go",
    /// while a genuine release must still register within a few frames.
    ///
    /// Before any repeat has been seen the window is [`KEY_HOLD_BASIC`], which
    /// is what a tap costs. That is the honest price of the fallback: the first
    /// ~half-second of holding a direction takes a step, pauses for the
    /// terminal's auto-repeat *delay*, then walks smoothly.
    fn hold_window(&self, mode: InputMode, idx: usize) -> Duration {
        match mode {
            InputMode::Enhanced => KEY_HOLD,
            InputMode::Basic => match self.repeat_gap[idx] {
                Some(g) => (g * 3).clamp(KEY_HOLD_BASIC_MIN, KEY_HOLD_BASIC_MAX),
                None => KEY_HOLD_BASIC,
            },
        }
    }

    /// How this session is getting frames to the terminal, for the status
    /// panel: the transport is chosen at runtime by asking the terminal, so it
    /// has to be *visible* or nobody can tell which one they got.
    ///
    /// `proto` is needed because "no [`TermImage`]" has two very different
    /// causes and calling both of them `legacy` misreports one of them. Sixel
    /// and half-blocks have no cell-addressed placement, so the resize path is
    /// the *only* path they have and there is nothing legacy about it — they are
    /// labelled `full`, for the whole picture going over the wire every frame,
    /// which is also the thing worth knowing about their cost. `legacy` is then
    /// only what it says: the `render_path = legacy` escape hatch.
    fn transport_label(&self, proto: ProtocolType) -> &'static str {
        match self.term_image.as_ref() {
            Some(t) if t.kitty_shm() => "shm",
            Some(_) => "png",
            None if matches!(proto, ProtocolType::Sixel | ProtocolType::Halfblocks) => {
                "full"
            }
            None => "legacy",
        }
    }

    /// Audio queue health, if a host stream is attached. Never blocks the
    /// render loop: a contended lock reports nothing rather than waiting.
    #[cfg(feature = "audio")]
    fn audio_sample(&self) -> Option<AudioSample> {
        let q = self.audio_queue.as_ref()?;
        let g = q.try_lock().ok()?;
        Some(AudioSample {
            queue_depth: g.len(),
            underruns: g.stats.underruns,
            dropped: g.stats.dropped,
            callbacks: g.stats.callbacks,
            sample_rate: self.audio_rate,
        })
    }

    /// Without the `audio` feature there is no host stream, so there is
    /// nothing to report — the telemetry record carries `"audio": null`.
    #[cfg(not(feature = "audio"))]
    fn audio_sample(&self) -> Option<AudioSample> {
        None
    }
}

#[cfg_attr(not(feature = "plugin-trainer"), allow(dead_code))]
struct App {
    config: Config,
    recent: Recent,
    picker: Picker,
    palette: Palette,
    /// Pixel-art upscaler applied to every frame before the image protocol
    /// (see [`crate::upscale`]); `]` cycles the algorithm, `}` the factor.
    ///
    /// Its `factor` is set per frame to the *transmission* factor
    /// ([`App::transmit_factor`]); the factor the user chose lives in
    /// `upscale_factor` below, because with `transmit_scale = auto` the two
    /// deliberately differ (a `nearest` upscale before transmission is work the
    /// terminal would redo for free).
    upscaler: Upscaler,
    /// The upscale factor the user selected, as shown in the status panel.
    upscale_factor: u8,
    screen: Screen,
    overlay: Overlay,
    fullscreen: bool,
    session: Option<Session>,
    /// Music mode's own state, present only once the shelf has been opened.
    /// See [`crate::screen::music_view::MusicState`].
    #[cfg(feature = "music")]
    music: Option<mv::MusicState>,
    should_quit: bool,
    /// How this terminal delivers keys. Set once by [`run`] from the kitty
    /// keyboard handshake; [`InputMode::Enhanced`] by default so the render
    /// tests (which never touch a terminal) exercise the normal path.
    input: InputMode,
    /// [`InputMode::Basic`] only: the last one-shot control key seen and when,
    /// so the terminal's auto-repeat stream cannot fire it thirty times a
    /// second. Unused in [`InputMode::Enhanced`], where `Repeat` is explicit.
    last_control: Option<(KeyCode, Instant)>,

    // Emulation controls (see the corresponding key handlers).
    /// Index into [`SPEED_LADDER`].
    speed: usize,
    /// Stats overlay toggle (off by default). Does not pause the game.
    stats: bool,
    /// Live trainer panel (`T`). Off by default, and while it is off the
    /// plugin reads no memory at all.
    trainer: bool,
    /// Per-Pokémon detail view inside the trainer panel (`d`).
    trainer_detail: bool,
    /// Whether a battle was on at the last draw, so the focus ring can be
    /// re-pointed at the opponent the moment one starts (and back at the party
    /// when it ends) instead of leaving a stale index meaning something else.
    trainer_fighting: bool,
    /// Set when something has happened that ratatui's cell diff cannot see —
    /// specifically a terminal image being taken off the screen, which leaves
    /// cells whose real contents no longer match the previous buffer. Consumed
    /// by the run loop, which clears the terminal before the next draw.
    full_redraw: bool,
    /// Selected party slot, cycled with Tab.
    trainer_sel: usize,
    /// How loudly a wild encounter's shiny-if-traded verdict is announced.
    shiny_alert: ShinyAlert,
    /// The last `TrainerPlugin::encounter_seq` this app reacted to, so the
    /// toast and the bell fire exactly once per Pokémon rather than every
    /// frame the banner is up.
    last_encounter_seq: u64,
    /// A terminal bell owed to a shiny-if-traded verdict, written on the next
    /// flush rather than from inside the emulation step.
    pending_bell: bool,
    /// The AI agent's commentary panel (`A`). On by default whenever an agent
    /// plugin is attached — the panel *is* the show; see
    /// [`crate::screen::agent_view`].
    agent_panel: bool,
    /// Active save-state slot (`1..=NUM_SLOTS`).
    slot: u32,
    /// Transient in-game notice (save/load/speed feedback) with its raise time.
    toast: Option<(String, Instant)>,
    /// The machine-fault toast has been raised once for this session.
    fault_announced: bool,

    // Picker screen state.
    entries: Vec<RomEntry>,
    /// Indices into `entries` that are "recently played" (shown first, starred).
    recent_flags: Vec<bool>,
    filter: String,
    filtering: bool,
    /// Visible (post-filter) indices into `entries`.
    visible: Vec<usize>,
    selected: usize,
    /// A transient message shown in the picker (e.g. a load error).
    message: Option<String>,
    /// Last layout values logged (dedupe `GB_DEBUG_LAYOUT` spam to changes).
    dbg_last: Option<(Rect, Rect, f32)>,
    /// Where the stats panel was last drawn, so its "never on top of the
    /// picture" invariant can be asserted directly rather than scraped.
    stats_rect: Option<Rect>,
    /// Where the trainer panel and its sprite image were last drawn, for the
    /// same invariant — the sprite is a second terminal image, so it must not
    /// share a cell with the game picture either.
    trainer_rect: Option<Rect>,
    sprite_rect: Option<Rect>,
    /// Where the commentary panel was last drawn — same invariant again.
    agent_rect: Option<Rect>,
    /// Signature of everything about the UI that can paint over the picture.
    /// When it changes, the image is retransmitted even if the frame did not,
    /// because the terminal's copy of it may have been partly overwritten.
    ui_sig: Option<UiSignature>,
    /// Machine-readable run telemetry; disabled (and free) unless configured.
    telemetry: Telemetry,
    /// What the *terminal* costs per displayed frame, measured by a round-trip
    /// fence when the window changes size. This is what stops `fps` from being
    /// a number that can only read 59: drawing is paced to it, so the frames we
    /// issue are frames the terminal can actually take. See
    /// [`crate::screen::termcost`].
    termcost: termcost::TerminalCost,
    /// Raw video-out; disabled (and free) unless configured. Driven from
    /// [`App::step_frame`] — **emulated** frames, not displayed ones, because
    /// display frames are deliberately dropped on a large window and a video
    /// stream may not be.
    video: VideoOut,
    /// Palette/upscale stages owned by the video stream, so `video_source =
    /// display` carries the on-screen look without disturbing the terminal's
    /// own buffers (whose upscale factor is the *transmission* factor, which
    /// is a different thing).
    video_prep: FramePrep,
    video_upscaler: Upscaler,
    /// The configured stream, kept unopened so `v` can arm and disarm it at
    /// any time. `None` when nothing was configured — `v` then records to a
    /// file named after the game (see [`App::start_video`]).
    video_template: Option<video::VideoSpec>,
    /// Should the stream come back up when a game is (re)loaded? Starts
    /// **false** — a configured destination is not a decision to stream (see
    /// [`open_video`]) — unless `video_autostart`/`--video-start` says
    /// otherwise, and then follows the user's last explicit `v` decision.
    video_autostart: bool,
    /// Is the live stream's destination named after the game? If so, loading a
    /// different ROM re-opens it under the new name; a plain file or FIFO the
    /// user chose keeps running across ROM switches (its consumer is attached
    /// to *that* path).
    video_named: bool,
    /// Complaints from config parsing, shown once so a mistyped key cannot
    /// fail silently.
    config_warnings: Vec<String>,
    /// Frames left on the "the cable is carrying traffic" indicator.
    ///
    /// A byte count alone cannot answer "is a trade happening *now*" — it only
    /// ever goes up. This is set whenever the count moves and decays over a
    /// second, so the panel distinguishes a connected-and-idle cable from one
    /// mid-exchange, which are different answers to "should I walk into the
    /// Cable Club?".
    link_busy_frames: u32,
    /// Transfer count at the previous frame, to notice movement.
    link_prev_transfers: u64,
    /// A cable discovered at startup, waiting for a game to attach it to.
    ///
    /// Discovery happens in [`run`] and **not** in `App::build`, deliberately:
    /// the render tests build an `App` per test in parallel, and a cable that
    /// advertised itself there would let two tests find each other. Same reason
    /// `App::build` does not apply the saved volume.
    pending_peer: Option<crate::link::peer::LinkPeer>,
    /// What happened when plugins were attached for the current session —
    /// shown in the help overlay so "is anything modifying my game?" always
    /// has an answer.
    plugin_notes: Vec<String>,
}

impl App {
    fn with_warnings(
        config: Config,
        recent: Recent,
        warnings: Vec<String>,
        video: VideoOut,
        video_template: Option<video::VideoSpec>,
        video_autostart: bool,
    ) -> App {
        // Playback volume. The config key is the *starting* level; whatever the
        // player last set with `,` / `.` / `m` is remembered and wins unless the
        // config file itself has changed since (see `VolumeState`). This is a
        // host-side attenuation of the finished sample stream — never NR50.
        //
        // Applied here rather than in `build` because `build` is what the
        // render tests construct, and a process-wide setting written by every
        // constructed `App` would fight tests running in parallel.
        let (vol_level, vol_muted) = VolumeState::resolve(config.volume);
        output_volume::set_level(vol_level);
        output_volume::set_muted(vol_muted);

        // Plug the cable in here, for the same reason as the volume above: a
        // cable advertises itself in a directory other processes read, and
        // `build` is what the render tests construct in parallel — two of them
        // would find *each other*. This constructor is the production one.
        //
        // It happens before any ROM is chosen so the port is live from the
        // first frame of the first game, which is what the Cable Club attendant
        // needs (`crate::link::peer`).
        let pending_peer = matches!(config.link, LinkSetting::Auto)
            .then(|| crate::link::peer::LinkPeer::discover("TERMINALGB", None));

        // Prefer the sharpest protocol the terminal advertises (build_picker),
        // then let an explicit config `protocol` override it.
        let mut app = App::build(
            config,
            recent,
            warnings,
            build_picker(),
            video,
            video_template,
        );
        app.video_autostart = video_autostart;
        app.pending_peer = pending_peer;
        if let Some(p) = &app.pending_peer {
            app.config_warnings
                .push(format!("link: cable {:?}", p.state()));
        }
        app
    }

    /// The rest of construction, with the [`Picker`] already chosen. Split out
    /// so tests can drive the real render path with a fixed protocol and cell
    /// size instead of querying a terminal that is not there.
    fn build(
        config: Config,
        recent: Recent,
        warnings: Vec<String>,
        picker: Picker,
        video: VideoOut,
        video_template: Option<video::VideoSpec>,
    ) -> App {
        let mut picker = picker;
        // HiDPI correction: on a display scaled by S, a pixel-addressed protocol
        // (iTerm2/sixel) renders our image in *physical* device pixels while the
        // terminal reported its cell size in *logical* pixels, so an image sized
        // to the reported grid comes out S times too small. Rebuild the picker
        // with a cell size multiplied by S, so both `fit_metrics` and the image
        // the protocol encodes are physical-pixel sized and fill the panel. The
        // default S is 1.0 (no change), so low-DPI / already-physical terminals
        // are untouched. See `CellScale`.
        let (qfw, qfh) = picker.font_size();
        let scale = resolve_cell_scale(config.cell_scale);
        if (scale - 1.0).abs() > 1e-3 {
            let sw = ((qfw as f32 * scale).round() as u16).max(1);
            let sh = ((qfh as f32 * scale).round() as u16).max(1);
            let proto = picker.protocol_type();
            let mut scaled = Picker::from_fontsize((sw, sh));
            scaled.set_protocol_type(proto);
            picker = scaled;
        }
        picker.set_background_color([0, 0, 0, 0]);
        if let Some(pref) = config.protocol.as_deref().and_then(protocol_from_str) {
            picker.set_protocol_type(pref);
        }
        let (fw, fh) = picker.font_size();
        debug_layout(&format!(
            "[picker] protocol={} queried_font=({qfw},{qfh}) cell_scale={scale:.3} \
             effective_font=({fw},{fh})",
            protocol_name(picker.protocol_type())
        ));
        let palette = config.palette;
        let shiny_alert = config.shiny_alert;
        let upscaler = Upscaler::new(config.upscale, config.upscale_factor);
        let config_factor = upscaler.factor();
        let config_algo = config.upscale;

        // Telemetry: `$GB_TELEMETRY` wins over the config key. Writing to
        // stdout from inside the alternate screen would corrupt the display, so
        // the terminal frontend refuses that destination and says why.
        let mut warnings = warnings;
        let dest = telemetry::Destination::from_env().or_else(|| {
            config
                .telemetry
                .as_deref()
                .and_then(telemetry::Destination::parse)
        });
        let telemetry = match dest {
            Some(telemetry::Destination::Stdout) => {
                warnings.push(
                    "telemetry: stdout would corrupt the terminal display — give a file \
                     path instead (telemetry = \"/tmp/gb.ndjson\")"
                        .to_string(),
                );
                Telemetry::disabled()
            }
            Some(d) => {
                let (t, err) = Telemetry::open(&d);
                if let Some(e) = err {
                    warnings.push(e);
                }
                t
            }
            None => Telemetry::disabled(),
        };

        let trainer_open = config.trainer;
        let mut app = App {
            config,
            recent,
            picker,
            palette,
            upscaler,
            upscale_factor: config_factor,
            screen: Screen::Picker,
            overlay: Overlay::None,
            fullscreen: false,
            session: None,
            #[cfg(feature = "music")]
            music: None,
            should_quit: false,
            input: InputMode::default(),
            last_control: None,
            speed: SPEED_NORMAL,
            stats: false,
            trainer: trainer_open,
            trainer_detail: false,
            trainer_fighting: false,
            full_redraw: false,
            trainer_sel: 0,
            shiny_alert,
            last_encounter_seq: 0,
            pending_bell: false,
            agent_panel: false,
            slot: 1,
            toast: None,
            fault_announced: false,
            entries: Vec::new(),
            recent_flags: Vec::new(),
            filter: String::new(),
            filtering: false,
            visible: Vec::new(),
            selected: 0,
            message: None,
            dbg_last: None,
            stats_rect: None,
            trainer_rect: None,
            agent_rect: None,
            sprite_rect: None,
            ui_sig: None,
            telemetry,
            termcost: terminal_cost_model(),
            video,
            video_prep: FramePrep::new(),
            video_upscaler: Upscaler::new(config_algo, config_factor),
            // Off. A configured destination says *where*, never *go* — the
            // explicit `video_autostart` opt-in is applied by
            // [`App::with_warnings`], and `v` is what goes live.
            video_autostart: false,
            video_named: false,
            video_template,
            config_warnings: warnings,
            link_busy_frames: 0,
            link_prev_transfers: 0,
            pending_peer: None,
            plugin_notes: Vec::new(),
        };
        app.rescan();
        if let Some(first) = app.config_warnings.first() {
            app.message = Some(first.clone());
        }
        app
    }

    /// (Re)build the ROM list from the configured directory, ordering
    /// recently-played entries first.
    fn rescan(&mut self) {
        let scanned = romlib::scan_dir(&self.config.rom_dir);
        let (entries, flags) = order_with_recent(scanned, &self.recent.paths);
        self.entries = entries;
        self.recent_flags = flags;
        self.filter.clear();
        self.filtering = false;
        self.selected = 0;
        self.recompute_visible();
    }

    fn recompute_visible(&mut self) {
        self.visible = romlib::filter_indices(&self.entries, &self.filter);
        if self.visible.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.visible.len() {
            self.selected = self.visible.len() - 1;
        }
    }

    /// Swap which of the two linked consoles is the one on screen and taking
    /// keys.
    ///
    /// Implemented as an actual swap of the two machines, so the renderer, the
    /// palette, the upscaler, the screenshot keys and the plugins all keep
    /// working on `Session::gb` and need no idea a second console exists. The
    /// cable records which side is driving the clock positionally, so it has to
    /// be told (`LinkCable::swap_ends`) or a byte in flight lands in the wrong
    /// machine.
    ///
    /// Buttons held on the console being left are released: a key the player
    /// let go of while looking elsewhere would otherwise stay down for ever.
    /// Tell the far console who is playing here.
    ///
    /// The **frontend** supplies the label and core stays game-agnostic: the
    /// cable relays bytes and has no business knowing what a trainer name is.
    /// The trainer plugin already identifies the cartridge and owns the Gen 1
    /// character map, so this reuses both rather than adding a second copy —
    /// and simply sends nothing for a cartridge it does not recognise.
    #[cfg(feature = "plugin-trainer")]
    fn publish_link_label(&mut self) {
        // `wPlayerName`, pokered `ram/wram.asm`. Already relied on by the
        // battle-control plugin, whose injected party has to inherit it or the
        // game treats every Pokémon as traded and disobedient.
        const W_PLAYER_NAME: u16 = 0xD158;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(p) = s.peer.as_mut() else { return };
        if !p.state().is_paired() {
            return;
        }
        let known = terminalgb_plugin_trainer::Game::detect(
            &s.gb.rom_title(),
            s.gb.global_checksum(),
        )
        .is_some();
        if !known {
            return;
        }
        let raw = s.gb.peek_range(W_PLAYER_NAME, 11);
        if terminalgb_plugin_trainer::text::looks_like_a_name(&raw) {
            p.set_label(&terminalgb_plugin_trainer::text::decode(&raw));
        }
    }

    #[cfg(not(feature = "plugin-trainer"))]
    fn publish_link_label(&mut self) {}

    /// Notice traffic on the wire and keep the indicator lit for a second
    /// after it stops. Also feeds the far end this player's in-game name, when
    /// something here can read one — "connected to RED" beats "connected".
    fn tick_link_activity(&mut self) {
        self.link_busy_frames = self.link_busy_frames.saturating_sub(1);
        let now = match self.session.as_ref() {
            Some(s) => match (s.link.as_ref(), s.peer.as_ref()) {
                (Some(l), _) => l.cable.stats().transfers,
                (None, Some(p)) => p.stats().transfers,
                _ => return,
            },
            None => return,
        };
        if now != self.link_prev_transfers {
            self.link_prev_transfers = now;
            self.link_busy_frames = 60;
        }
        self.publish_link_label();
    }

    /// The one description of the cable, used by the Status row **and** the
    /// trainer panel. One mechanism, two places — the captain watches the
    /// trainer panel while playing and the Status row while debugging, and two
    /// independent renderings of the same fact would drift.
    ///
    /// Returns `(short, long)`: `short` fits the 18-column Status row, `long`
    /// is for the trainer panel, which has more width to spend. Both name the
    /// three states that actually matter to somebody deciding whether to walk
    /// into a Cable Club: **nobody there**, **someone there**, and **a trade
    /// actually happening**.
    fn link_summary(&self) -> Option<(String, String)> {
        use crate::link::peer::PeerState;
        let s = self.session.as_ref()?;

        // The in-process second console: always "connected", by construction.
        if let Some(l) = s.link.as_ref() {
            let n = l.cable.stats().transfers;
            let fault = if l.cable.desynced() { " FAULT" } else { "" };
            return Some((
                format!("{} {n}{fault}", truncate(&l.title, 11)),
                format!("linked to {} · {n} bytes{fault}", l.title),
            ));
        }

        let p = s.peer.as_ref()?;
        let busy = p.stats().transfers;
        Some(match p.state() {
            PeerState::Off => ("off".into(), "cable unplugged (W)".into()),
            PeerState::Waiting => ("waiting".into(), "no one connected".into()),
            PeerState::Unplugged => ("gone".into(), "the other console left".into()),
            PeerState::Paired {
                title,
                label,
                same_save,
            } => {
                let who = if label.is_empty() {
                    title.clone()
                } else {
                    format!("{label} · {title}")
                };
                // "Connected" and "trading right now" are different answers.
                // `wLinkState` is the game's own view and is what the trainer
                // panel keys off; here the byte count is enough to separate a
                // live exchange from an idle cable.
                let active = self.link_active();
                let warn = if *same_save { "  SAME SAVE!" } else { "" };
                (
                    format!(
                        "{} {busy}{}",
                        truncate(title, 10),
                        if active { "*" } else { "" }
                    ),
                    format!(
                        "connected to {who}{}{warn}",
                        if active { " — TRADING" } else { "" }
                    ),
                )
            }
        })
    }

    /// Is the link carrying traffic *right now*, as opposed to merely being
    /// plugged in? Read from the game rather than from the cable: Gen 1's
    /// `wLinkState` says whether the player is in a Cable Club room or actually
    /// mid-trade, which is the distinction a player cares about.
    fn link_active(&self) -> bool {
        self.link_busy_frames > 0
    }

    /// `W` — the link key. What it does depends on which kind of cable is
    /// attached, and the two kinds are mutually exclusive (a Game Boy has one
    /// port), so it can never be ambiguous:
    ///
    /// * a discovered peer (`link = auto`, the default) → **unplug / plug in**,
    ///   which is what somebody running several games needs;
    /// * a second console in this process (`link = <rom>`) → **swap which one
    ///   you are playing**.
    fn link_key(&mut self) {
        if self.session.as_ref().is_some_and(|s| s.peer.is_some()) {
            self.toggle_peer_cable();
        } else {
            self.swap_link_focus();
        }
    }

    /// Take this console off the wire on the way out.
    fn unplug_link(&mut self) {
        if let Some(s) = self.session.as_mut() {
            if let Some(p) = s.peer.as_mut() {
                p.close();
            }
        }
        if let Some(p) = self.pending_peer.as_mut() {
            p.close();
        }
    }

    /// Unplug the discovered cable, or plug it back in.
    ///
    /// Unplugging is a real disconnect — the far console sees a clean goodbye,
    /// not a crash, so neither save is poisoned. Plugging back in runs
    /// discovery again, so it finds whoever is available now rather than
    /// insisting on the console it had before.
    fn toggle_peer_cable(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(p) = s.peer.as_mut() else { return };
        if matches!(p.state(), crate::link::peer::PeerState::Off) {
            let title = s.gb.rom_title();
            **p = crate::link::peer::LinkPeer::discover(&title, None);
            self.set_toast("link: cable plugged in");
        } else {
            p.close();
            **p = crate::link::peer::LinkPeer::off();
            self.set_toast("link: cable unplugged");
        }
    }

    fn swap_link_focus(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(l) = s.link.as_mut() else {
            self.set_toast("no link cable — `link = auto` or `link = <rom>`");
            return;
        };
        for i in 0..8 {
            if s.held[i].is_some() {
                s.gb.keyup(key_from_index(i));
                s.held[i] = None;
                s.last_press[i] = None;
            }
        }
        std::mem::swap(&mut s.gb, &mut l.gb);
        l.cable.swap_ends();
        l.title = l.gb.rom_title();
        let (now, other) = (s.gb.rom_title(), l.title.clone());
        self.set_toast(format!("link: now playing {now} (other console: {other})"));
    }

    /// Boot the second console, if `link = <rom>` is configured, and hand back
    /// the wire joining it to the one being started.
    ///
    /// Failure is a **warning, not a refusal** — a missing or unreadable second
    /// cartridge costs you the cable and nothing else, the same rule the rest of
    /// this frontend follows for an absent capability.
    fn open_link_partner(&mut self) -> Option<Box<LinkPartner>> {
        let crate::screen::config::LinkSetting::Rom(rom) = &self.config.link else {
            return None;
        };
        let path = PathBuf::from(rom);
        let data = match std::fs::read(&path) {
            Ok(d) if d.len() >= 0x150 => d,
            Ok(_) => {
                self.config_warnings
                    .push(format!("link: {} is too small to be a ROM", path.display()));
                return None;
            }
            Err(e) => {
                self.config_warnings
                    .push(format!("link: cannot read {}: {e}", path.display()));
                return None;
            }
        };
        // The partner console gets the same optional boot ROM as the first —
        // two real consoles either both have one or neither does, and "my boot
        // ROM runs on one screen and not the other" would be a surprise nobody
        // could diagnose. See [`crate::boot`].
        let model = crate::mode::Model::Auto;
        let boot = crate::boot::load(model.resolve(data[0x0143]));
        let mut gb = Gameboy::new_with_boot_rom(data, Some(path.clone()), model, boot);
        let title = gb.rom_title();
        // **The skew is not a nicety.** Two real consoles never share a crystal
        // and two real players never press A on the same frame, so their link
        // handshakes drift past each other until they interleave. Two cores
        // stepped by one loop have no drift at all: started together they stay
        // in phase for ever, both driving the clock at the same moment and each
        // finding the other deaf. Measured on two Pokémon Blues — zero skew
        // never connects, any skew from one frame up connects within a few
        // hundred. See `docs/link-cable.md` §"Phase".
        for _ in 0..LINK_START_SKEW {
            gb.frame();
        }
        self.config_warnings.push(format!(
            "link: cable attached to {title} ({})",
            path.display()
        ));
        Some(Box::new(LinkPartner {
            gb,
            cable: crate::link::LinkCable::new(),
            title,
        }))
    }

    /// Load a ROM and switch to the Playing screen. On failure, stay on the
    /// picker and show why.
    fn start_game(&mut self, path: &Path) {
        let term_image = self.new_term_image();
        let sprite_image = self.new_sprite_image();
        let team_image = self.new_team_image();
        match load_session(path, &self.config) {
            Ok((mut gb, entry, audio, plugin_notes)) => {
                self.recent.record(path);
                self.recent.save();
                // Carry the current speed rung's audio contract into the new
                // session (see bump_speed): non-1x must drop, not wait.
                gb.set_audio_fast_forward(self.speed != SPEED_NORMAL);
                // Plugins were attached by `load_session`; the machine owns the
                // single host (see `crate::plugin`). All the frontend does here
                // is tell the trainer view whether its panel is on screen.
                #[cfg(feature = "plugin-trainer")]
                if let Some(t) = gb.plugin_mut::<TrainerPlugin>() {
                    t.set_active(self.trainer);
                }
                self.trainer_sel = 0;
                // If an agent is driving, its commentary is the show — open the
                // panel without being asked. `A` closes it again.
                #[cfg(feature = "plugin-agent")]
                if gb
                    .plugin::<terminalgb_plugin_agent::AgentPlugin>()
                    .is_some()
                {
                    self.agent_panel = true;
                }
                #[cfg(feature = "audio")]
                let (audio_queue, audio_rate) = (audio.queue.clone(), audio.rate);
                let link = self.open_link_partner();
                // One port: an in-process second console and a discovered peer
                // are alternatives, never both.
                let peer = if link.is_some() {
                    None
                } else {
                    self.pending_peer.take().map(|mut p| {
                        // The cable was plugged in before a cartridge was
                        // chosen; tell the far end what it is now, and which
                        // save file this console writes, so two copies of one
                        // game can be flagged.
                        p.set_identity(
                            &gb.rom_title(),
                            Some(&path.with_extension("sav")),
                        );
                        Box::new(p)
                    })
                };
                self.session = Some(Session::new(
                    gb,
                    entry,
                    link,
                    peer,
                    term_image,
                    sprite_image,
                    team_image,
                    #[cfg(feature = "audio")]
                    audio_queue,
                    #[cfg(feature = "audio")]
                    audio_rate,
                    #[cfg(feature = "audio")]
                    audio.stream,
                    #[cfg(not(feature = "audio"))]
                    audio,
                ));
                self.screen = Screen::Playing;
                self.overlay = Overlay::None;
                self.fullscreen = false;
                self.message = None;
                set_window_title(self.session.as_ref().map(|s| s.entry.title.as_str()));
                // A stream named after the game it carries must restart under
                // the new name rather than appending a second game to the
                // first one's file. A user-chosen path is left alone.
                if self.video.is_enabled() && self.video_named {
                    self.video.flush();
                    self.video = VideoOut::disabled();
                }
                if self.video_autostart {
                    self.start_video_as(false);
                }
                // Config problems follow you into the game — silently ignoring
                // a key the user set is the bug being fixed here. A plugin
                // attaching (or failing to) is at least as important to say.
                if let Some(n) = plugin_notes.first().cloned() {
                    self.set_toast(n);
                } else if let Some(w) = self.config_warnings.first().cloned() {
                    self.set_toast(format!("⚠ {w}"));
                } else {
                    // Audio asked for and refused. This build *can* play sound,
                    // so silence here is a fault, not a setting — say so once
                    // rather than leaving the player to guess.
                    let audio = crate::sound::host_status::snapshot();
                    if audio.state == crate::sound::host_status::State::Failed {
                        self.set_toast(format!("⚠ Audio {}", audio.label()));
                    }
                }
                self.plugin_notes = plugin_notes;
            }
            Err(e) => {
                self.screen = Screen::Picker;
                self.message = Some(format!("Failed to load {}: {e}", path.display()));
            }
        }
    }

    /// Return to the picker, dropping the current session (refresh recents).
    fn back_to_picker(&mut self) {
        self.session = None;
        self.screen = Screen::Picker;
        self.overlay = Overlay::None;
        self.recent = Recent::load();
        self.rescan();
    }

    // ---- main loop ----

    fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), Box<dyn Error>> {
        // Deadline accumulator, NOT `last_tick = Instant::now()` after each
        // frame. Resetting to `now` adds the step/draw/poll overhead of every
        // loop iteration to the frame period, so emulation runs several
        // percent slow on average — and audio sample production is slaved to
        // emulation speed, so a slow producer drains the audio queue and
        // underruns ("blocky" sound). Advancing the deadline by exactly
        // FRAME_TIME keeps the long-run average at 59.7275 Hz; a bounded
        // catch-up (extra emulated frames, cheap next to terminal drawing)
        // absorbs slow draws and late wakeups.
        let mut next_frame = Instant::now() + FRAME_TIME;
        // Rendering is paced *separately* from emulation. `next_draw` is
        // pushed out by however long the last draw actually took, so a terminal
        // that cannot repaint at 60 Hz simply gets fewer pictures — it never
        // makes the emulator (and therefore the game, and the audio clock that
        // is slaved to it) run slow. Display frames are dropped, not emulated
        // ones. See the frames_dropped counter and docs/display.md.
        let mut next_draw = Instant::now();
        let mut dirty = true;

        while !self.should_quit {
            // 1. Advance emulation, paced to the selected speed (1x = real
            //    time; faster rungs shorten the interval; MAX runs a burst).
            #[cfg(feature = "music")]
            let on_a_machine = matches!(self.screen, Screen::Playing | Screen::Shelf);
            #[cfg(not(feature = "music"))]
            let on_a_machine = matches!(self.screen, Screen::Playing);
            if on_a_machine {
                // Music mode's shelf is filled by a worker thread; take what it
                // has finished without ever waiting for it.
                #[cfg(feature = "music")]
                if let Some(m) = self.music.as_mut() {
                    if m.lib.poll() {
                        dirty = true;
                    }
                }
                #[cfg(feature = "music")]
                if self.autoplay_when_ready() {
                    dirty = true;
                }
                #[cfg(feature = "music")]
                if self.collect_export() {
                    dirty = true;
                }
                let running = self
                    .session
                    .as_ref()
                    .map(|s| !s.paused && self.overlay == Overlay::None)
                    .unwrap_or(false);
                if !running {
                    // Paused / overlay: don't accumulate frame debt.
                    next_frame = Instant::now() + FRAME_TIME;
                } else {
                    let level = &SPEED_LADDER[self.speed];
                    if level.uncapped {
                        // MAX is deliberately unpaced: run a bounded burst,
                        // and don't carry frame debt into the capped rungs.
                        for _ in 0..level.burst {
                            self.step_frame();
                        }
                        next_frame = Instant::now() + FRAME_TIME;
                        dirty = true;
                    } else if Instant::now() >= next_frame {
                        // Deadline accumulator: advance by the rung's period
                        // so the long-run average rate is exact. Resetting to
                        // `now` instead would add the step/draw/poll overhead
                        // of every loop iteration to the period — emulation
                        // runs slow, audio production (slaved to it) starves
                        // the queue, and playback goes "blocky". Catch-up is
                        // bounded by MAX_CATCHUP so a long stall resyncs
                        // instead of fast-forwarding the game.
                        let interval = level.interval();
                        let (due, deadline) =
                            frames_due(Instant::now(), next_frame, interval);
                        next_frame = deadline;
                        for _ in 0..due {
                            self.step_frame();
                        }
                        dirty = true;
                    }
                }
                // 2. Auto-release keys that were not refreshed.
                if self.release_expired() {
                    dirty = true;
                }
                // 2b. Hold the video stream's wall-clock rate even when no
                //     emulated frame was produced (paused, modal overlay): a
                //     consumer that stops receiving bytes stalls, and a
                //     recording across a pause would come out short.
                self.video_tick(false);
            }

            // 3. Draw when something changed *and* the display is due. On a
            //    large window a draw can cost more than a frame; rather than
            //    stalling emulation behind it we simply draw less often.
            let now = Instant::now();
            if dirty && now >= next_draw {
                let t = Instant::now();
                // A picture that has just been taken off the screen leaves
                // cells whose *terminal* contents no longer match ratatui's
                // idea of them, and the diff will not repaint a cell it thinks
                // is already right. One full repaint costs a frame's worth of
                // text and happens only on the transition.
                if self.take_full_redraw() {
                    terminal.clear()?;
                }
                terminal.draw(|f| self.render(f))?;
                // The picture goes out *after* the text frame has been
                // flushed, so it is never clipped, re-diffed or overwritten by
                // `ratatui`. Nothing is written when the frame was unchanged.
                let issued = self.flush_image()?;
                let our_cost = t.elapsed();
                // Measure the terminal's own share of the frame — decode,
                // upload and composite — which none of the timers above can
                // see, and which grows with the composited *area* while our own
                // cost and the bytes on the wire stay flat (measured 0.04 ms →
                // 2.97 ms over a 25× area range on WezTerm; the ladder is in
                // `crate::screen::termcost`). Blocking, so it runs only when
                // the window has changed size or the figure has gone stale,
                // never in the steady state.
                if issued {
                    let layout = self
                        .session
                        .as_ref()
                        .map(|s| s.disp_cells)
                        .unwrap_or((0, 0));
                    self.termcost.observe(Instant::now(), layout, fence);
                }
                // Never faster than the panel refresh, never slower than either
                // the draw or **the terminal** can sustain. Issuing pictures a
                // terminal cannot take does not make them appear — it coalesces
                // them, which costs us the work and costs the user nothing but
                // latency, while our counter cheerfully reads 59.
                let interval = self.termcost.draw_interval(FRAME_TIME);
                next_draw = next_draw_at(t, our_cost.max(interval));
                dirty = false;
                self.emit_telemetry(our_cost);
            }

            // 4. Poll input until the next thing is due, so the loop sleeps
            //    instead of spinning but still reacts to a keypress at once.
            //    `poll` returns as soon as a key arrives, so a longer timeout
            //    costs nothing in responsiveness — only in how promptly the
            //    next emulated frame starts, which is why it is bounded.
            let now = Instant::now();
            let timeout = match self.screen {
                // Nothing is due on a clock: wait on input alone.
                Screen::Picker => IDLE_POLL,
                // The shelf runs a real machine so the music plays, so it is
                // paced exactly like the game screen.
                #[cfg(feature = "music")]
                Screen::Shelf => Duration::from_millis(4),
                Screen::Playing => {
                    let deadline = if dirty {
                        next_frame.min(next_draw)
                    } else {
                        next_frame
                    };
                    deadline
                        .saturating_duration_since(now)
                        .clamp(Duration::from_millis(1), Duration::from_millis(8))
                }
            };
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) => {
                        self.handle_key(key);
                        dirty = true;
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
        }
        self.telemetry.flush();
        self.video.flush();
        self.recent.save();
        Ok(())
    }

    /// Advance the machine exactly one emulated frame. **Emulation only** — no
    /// palette mapping, no upscaling, no encoding. Those used to happen here,
    /// which is what coupled the game's speed to the terminal's; they now
    /// happen in [`App::render_display`], at most once per displayed frame.
    fn step_frame(&mut self) {
        if let Some(s) = self.session.as_mut() {
            let t = Instant::now();
            // `Gameboy::frame` publishes the "frame rendered" event to any
            // attached plugin itself, so there is nothing to drive from here.
            //
            // With a link cable attached the cable owns the stepping instead:
            // it interleaves the two cores instruction by instruction, which is
            // the only way a byte on the wire lands on the right cycle at both
            // ends. It performs the same per-frame housekeeping `frame` does,
            // for both machines.
            match (s.link.as_mut(), s.peer.as_mut()) {
                (Some(l), _) => l.cable.run_frame(&mut s.gb, &mut l.gb),
                (None, Some(p)) => p.run_frame(&mut s.gb),
                (None, None) => s.gb.frame(),
            }
            s.t_emulate = telemetry::ms(t.elapsed());
            s.emu_count += 1;
            s.emu_frames += 1;
            s.skipped_since_display += 1;
            let elapsed = s.fps_mark.elapsed();
            if elapsed >= Duration::from_secs(1) {
                let secs = elapsed.as_secs_f32();
                s.fps = s.disp_count as f32 / secs;
                s.emu_fps = s.emu_count as f32 / secs;
                // Per *displayed* frame, so it is comparable with the byte
                // figures in docs/rendering.md — deduplicated frames are
                // displayed frames that cost nothing, and averaging over them
                // is exactly the honest picture.
                s.tx_per_frame = if s.disp_count > 0 {
                    s.tx_window as f32 / s.disp_count as f32
                } else {
                    0.0
                };
                s.tx_window = 0;
                s.disp_count = 0;
                s.emu_count = 0;
                s.fps_mark = Instant::now();
            }
        }
        self.tick_link_activity();
        // Feed the video stream from the **emulated** frame, not the displayed
        // one: display frames are deliberately dropped on a large window
        // (see `crate::termimage`) and a video stream may not be.
        self.video_tick(true);
        self.announce_new_encounter();
        self.check_fault();
    }

    /// Tell the player, once, when the emulator has stopped writing their save.
    ///
    /// A crashed game can no longer reach the battery file (see
    /// [`crate::mmu::MemoryManagementUnit::report_fault`]) — but a guard that
    /// protects the save silently is one the player cannot act on, and the
    /// action is theirs: restart, and the untouched `.sav` is waiting. The
    /// Status panel keeps a `Fault` row for as long as it lasts.
    fn check_fault(&mut self) {
        if self.fault_announced {
            return;
        }
        let Some(reason) = self.session.as_ref().and_then(|s| s.gb.fault()) else {
            return;
        };
        let msg = format!(
            "The game has crashed ({reason}). Your save file is NOT being written \
             — restart to recover."
        );
        self.fault_announced = true;
        self.set_toast(msg);
    }

    /// The crash reason, for the Status panel.
    fn fault_text(&self) -> Option<&str> {
        self.session.as_ref().and_then(|s| s.gb.fault())
    }

    /// Raise the one-shot half of the shiny alert.
    ///
    /// The banner is *state* — it is redrawn from the latched encounter for as
    /// long as the Pokémon is on screen. This is the *edge*: the toast and the
    /// terminal bell that fire once, at the instant it appears, because that is
    /// the moment the captain is looking at the game and not at a panel.
    ///
    /// The whole cost when nothing is happening is comparing two `u64`s.
    #[cfg(feature = "plugin-trainer")]
    fn announce_new_encounter(&mut self) {
        let last = self.last_encounter_seq;
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let Some(plugin) = session.gb.plugin::<TrainerPlugin>() else {
            return;
        };
        let seq = plugin.encounter_seq();
        if seq == last {
            return;
        }
        self.last_encounter_seq = seq;
        let Some(enc) = plugin.encounter() else {
            return;
        };
        if !enc.would_be_shiny() || self.shiny_alert == ShinyAlert::Off {
            return;
        }
        let msg = format!(
            "\u{2605} {} L{} WOULD BE SHINY IF TRADED TO GEN 2 \u{2605} DV {}",
            enc.name,
            enc.level,
            enc.dv_string()
        );
        if self.shiny_alert.rings() {
            // BEL. The one moment in this program worth interrupting someone
            // for: the verdict is only useful before a Ball is spent.
            self.pending_bell = true;
        }
        self.set_toast(msg);
    }

    #[cfg(not(feature = "plugin-trainer"))]
    /// No trainer plugin is built in, so there is no encounter to announce.
    fn announce_new_encounter(&mut self) {}

    /// The wild encounter to draw a banner for this frame, if any. `None` in the
    /// overworld, in a trainer battle, and when the alert is configured off.
    #[cfg(feature = "plugin-trainer")]
    fn banner_encounter(
        &self,
    ) -> Option<&terminalgb_plugin_trainer::encounter::WildEncounter> {
        if !self.shiny_alert.shows_banner() {
            return None;
        }
        self.session
            .as_ref()?
            .gb
            .plugin::<TrainerPlugin>()?
            .encounter()
    }

    /// `0` no banner, `1` an ordinary encounter, `2` a shiny-if-traded one.
    /// Part of [`App::ui_signature`] because the row changes the layout, and a
    /// layout change means every terminal image may have been painted over.
    #[cfg(feature = "plugin-trainer")]
    fn banner_state(&self) -> u8 {
        match self.banner_encounter() {
            None => 0,
            Some(e) if e.would_be_shiny() => 2,
            Some(_) => 1,
        }
    }

    #[cfg(not(feature = "plugin-trainer"))]
    /// Without the trainer plugin there is never a banner row.
    fn banner_state(&self) -> u8 {
        0
    }

    /// Draw the encounter banner into a row of its own. Never over the picture,
    /// for the reason given on [`App::render_stats`].
    #[cfg(feature = "plugin-trainer")]
    fn render_encounter_banner(&self, f: &mut Frame<'_>, area: Rect) {
        let Some(enc) = self.banner_encounter() else {
            return;
        };
        if area.width == 0 || area.height == 0 {
            return;
        }
        let row = Rect::new(area.x, area.y, area.width, 1);
        f.render_widget(Clear, row);
        f.render_widget(
            Paragraph::new(tv::encounter_banner(enc, area.width as usize)),
            row,
        );
    }

    #[cfg(not(feature = "plugin-trainer"))]
    fn render_encounter_banner(&self, _f: &mut Frame<'_>, _area: Rect) {}

    /// Start or stop the raw video stream while the game runs (`v`).
    ///
    /// The captain's rule: *never be unsure whether you are live*. So the
    /// Status panel carries a `Video` row, the toggle raises a toast, and the
    /// full `ffmpeg` command lands in the `?` overlay's notices — and all three
    /// name the **intent** (`LIVE` for a consumer, `REC` for a file) and the
    /// destination, because "recording to ~/Videos" and "feeding the camera"
    /// looked identical before and one of them was silently the wrong one.
    ///
    /// **`v` is the live feed and only ever the live feed.** With no live
    /// destination configured it says so instead of quietly recording a file —
    /// that fallback is why "stop the stream" once answered `REC stopped: 268
    /// frames → ~/Videos/gameboy` to somebody thinking about his camera. `V`
    /// records, always, and the two never trade places.
    fn toggle_video(&mut self) {
        if self.stop_video() {
            return;
        }
        self.start_video_as(false);
        self.video_autostart = self.video.is_enabled();
    }

    /// Record to a timestamped file in [`video_dir`], regardless of any
    /// `video_out` destination (`V`).
    ///
    /// Deliberately its own key. Recording something to keep and feeding a live
    /// consumer are different jobs with different destinations and different
    /// formats, and sharing one key meant a configured camera bridge made
    /// "record a clip" unreachable — and, before that, made "feed the camera"
    /// silently write a 160×144 file nobody was reading.
    fn toggle_recording(&mut self) {
        if self.stop_video() {
            return;
        }
        let template = self.video_template.take();
        self.start_video_as(true);
        self.video_template = template;
        self.video_autostart = false;
    }

    /// Stop whatever is running. `true` if there was something to stop.
    ///
    /// A **recording is closed** — that is what makes it a finished file. A
    /// **live destination keeps existing** and switches to the idle card: the
    /// device is a persistent output and streaming the game into it is a mode,
    /// so `v` must never make a viewer lose the camera mid-call. See
    /// [`VideoOut::go_idle`].
    fn stop_video(&mut self) -> bool {
        if !self.video.is_enabled() {
            return false;
        }
        let s = self.video.stats();
        let what = self.video.intent().word();
        let dest = self.video.destination();
        self.video.flush();
        let held = self.video.must_stay_open();
        if held {
            self.video.go_idle();
        } else {
            self.video = VideoOut::disabled();
        }
        self.video_autostart = false;
        self.set_toast(format!(
            "{what} stopped: {} frames, {:.1} MB → {dest}{}",
            s.frames,
            s.bytes as f64 / 1_048_576.0,
            if held {
                " — idle card, `v` resumes"
            } else {
                ""
            },
        ));
        true
    }

    /// `recording` is `V`'s job (a file to keep); `false` is `v`'s (a live
    /// consumer). They are separate on purpose — see [`App::toggle_video`].
    fn start_video_as(&mut self, recording: bool) {
        let Some(s) = self.session.as_ref() else {
            self.set_toast("Video: load a game first".to_string());
            return;
        };
        let title = s.entry.title.clone();
        let (mut spec, named) = match self.video_template.clone() {
            Some(t) => {
                let named = needs_title(&t.dest);
                (t, named)
            }
            // Nothing configured. `v` used to quietly *record a file* here,
            // which is how "stop the stream" printed `REC stopped: 268 frames
            // → ~/Videos/gameboy` at somebody who was thinking about a camera.
            // Recording and feeding a live consumer are different jobs; `v` is
            // only ever the live one, and it says what to do instead rather
            // than doing the other thing.
            None if !recording => {
                self.set_toast(
                    "No live destination: set video_out (e.g. /dev/video10), or \
                     press V to record a file"
                        .to_string(),
                );
                return;
            }
            None => (
                video::VideoSpec::new(video::Destination::Path(video_dir(&self.config))),
                true,
            ),
        };
        // `source = display` fixes its geometry from the upscaler in force
        // right now; later changes are letterboxed into it (see `crate::video`).
        if spec.source == VideoSource::Display && spec.size.is_none() {
            let (w, h) = Upscaler::new(self.upscaler.algorithm(), self.upscale_factor)
                .output_size(160, 144);
            spec.size = Some((w as u32, h as u32));
        }
        // `as_dir`: the fallback destination is the recordings *directory*,
        // which may not exist yet, so it cannot be detected by `is_dir`.
        let as_dir = self.video_template.is_none();
        spec.dest = match self.resolve_destination(&spec, &title, as_dir) {
            Ok(d) => d,
            Err(e) => {
                self.set_toast(format!("Video failed: {e}"));
                return;
            }
        };
        // The game's name travels with the stream: it is what names the
        // sidecar's `title=`, and through that the virtual camera's label.
        spec.title = Some(title.clone());
        let short = match &spec.dest {
            video::Destination::Stdout => "stdout".to_string(),
            video::Destination::Path(p) => p.display().to_string(),
        };
        // An idling destination is *resumed*, never reopened: this is the whole
        // point of holding it (see [`VideoOut::go_idle`]) — reopening a camera
        // somebody is watching fails, and would fail silently into "waiting".
        if self.video.is_idle() && self.video.spec().dest == spec.dest {
            self.video.resume();
            self.video_named = named;
            let t = waiting_toast(&self.video, &short);
            self.set_toast(t);
            return;
        }
        let (v, note) = VideoOut::open(spec);
        // A note is not necessarily a failure any more: an armed FIFO with no
        // consumer yet returns one *and* a working stream.
        if !v.is_enabled() {
            let e = note.unwrap_or_else(|| "video: could not start".to_string());
            // The V4L2 refusal is several lines of instructions; the toast gets
            // the headline and the `?` overlay gets all of it.
            self.set_toast(format!(
                "Video failed: {}",
                e.lines().next().unwrap_or("see ? for details")
            ));
            self.config_warnings.push(e);
            return;
        }
        let t = waiting_toast(&v, &short);
        self.set_toast(t);
        if let Some(n) = note {
            // A note on a stream that *did* open is a quality warning about the
            // live feed — "this camera is the wrong shape for a video call", say
            // — and burying it in `?` is no use to somebody who just pressed
            // `v`. It replaces the status toast, because a warning outranks a
            // restatement of the destination they just chose.
            self.set_toast(format!(
                "⚠ {}",
                n.lines().next().unwrap_or("see ? for details")
            ));
            self.config_warnings.push(n);
        }
        self.config_warnings.push(v.describe());
        if self.video.is_idle() {
            // Only one destination is held at a time, so taking a different one
            // gives the camera up — and a camera given up cannot be taken back
            // while anything is watching it (see [`VideoOut::go_idle`]). Say so
            // rather than letting it disappear.
            self.config_warnings.push(format!(
                "video: released the camera at {} to open {short}. Getting it \
                 back needs every viewer closed first.",
                self.video.destination(),
            ));
        }
        self.video = v;
        self.video_named = named;
        // The camera the user says they are feeding must name the game — see
        // `video::v4l2::check_label` for why this can only be checked, not
        // fixed.
        if let Some(dev) = self.config.video_device.clone() {
            if let Some(w) = video::v4l2::check_label(Path::new(&dev), &title) {
                self.set_toast(
                    "⚠ camera is labelled for a different game — see ?".to_string(),
                );
                self.config_warnings.push(w);
            }
        }
    }

    /// Turn a configured destination into the concrete one for *this game*:
    /// `%t`/`%s` placeholders expanded, and a directory turned into a file
    /// named after the cartridge. A FIFO or plain file is left exactly as the
    /// user wrote it.
    fn resolve_destination(
        &self,
        spec: &video::VideoSpec,
        title: &str,
        as_dir: bool,
    ) -> Result<video::Destination, String> {
        let video::Destination::Path(p) = &spec.dest else {
            return Ok(video::Destination::Stdout);
        };
        let stamp = now_stamp();
        let expanded = PathBuf::from(video::expand_placeholders(
            &p.to_string_lossy(),
            title,
            &stamp,
        ));
        let ends_in_sep = p.to_string_lossy().ends_with('/');
        if !as_dir && !ends_in_sep && !expanded.is_dir() {
            return Ok(video::Destination::Path(expanded));
        }
        // A directory: name the recording after the game, and carry the two
        // things `rawvideo` cannot (geometry, pixel format) in the filename.
        std::fs::create_dir_all(&expanded)
            .map_err(|e| format!("{}: {e}", expanded.display()))?;
        let (w, h) = spec.size.unwrap_or((video::SRC_W, video::SRC_H));
        let name = video::recording_name(title, &stamp, w, h, spec.format);
        Ok(video::Destination::Path(expanded.join(name)))
    }

    /// The Status panel's video row: the answer to "am I live?".
    /// One line for the Status panel's `Audio` row: `off`, `● LIVE 48000Hz ×2`,
    /// or `failed: <the cpal error>`.
    ///
    /// Without the `audio` feature this is always `off`, which is the truth —
    /// that build has no host player at all.
    fn audio_status(&self) -> String {
        crate::sound::host_status::snapshot().label()
    }

    /// The Status panel's `Video` row: **off**, **waiting for a consumer**, or
    /// **live** — and which of the two jobs it is doing.
    ///
    /// The three states are not decoration. A FIFO with nothing reading it is
    /// armed but delivering nothing, and calling that `LIVE` is precisely what
    /// let a camera bridge look like it was working while the frames went into
    /// a file somewhere else entirely. The destination is in the row for the
    /// same reason: `REC` and `LIVE` answer *what*, the path answers *where*.
    fn video_status(&self) -> String {
        match self.video.state() {
            // Idling: the *game* is not going out, but the camera still exists
            // and is being fed the card. Both halves matter to whoever is
            // reading the row — "no picture from me" and "the device is still
            // mine" — so neither is hidden behind a bare `off`.
            video::StreamState::Off if self.video.is_idle() => {
                format!("◍ idle {}", tail_of(&self.video.destination()))
            }
            video::StreamState::Off => "off".to_string(),
            video::StreamState::Waiting => {
                format!("◌ wait {}", tail_of(&self.video.destination()))
            }
            video::StreamState::Live => {
                let s = self.video.stats();
                format!(
                    "● {} {:.0}M {}",
                    self.video.intent().word(),
                    s.bytes as f64 / 1_048_576.0,
                    tail_of(&self.video.destination()),
                )
            }
        }
    }

    /// Write the current frame to a PNG and say where it went.
    ///
    /// `raw` selects the **160×144 framebuffer**; the default (`c`) is **what
    /// the player is looking at** — palette-mapped and run through the
    /// upscaler, because that is what "screenshot" means to a person, and
    /// because the four upscalers and three palettes are precisely the thing
    /// worth capturing and comparing.
    ///
    /// The upscale factor used is the one *selected* (`}`), not the
    /// transmission factor: with `transmit_scale = auto` a `nearest` upscale is
    /// left to the terminal (see [`App::transmit_factor`]), which is right for
    /// the wire and wrong for a file.
    ///
    /// Same frame source as the video stream ([`App::video_tick`]) — one path
    /// writes a file, the other streams.
    fn screenshot(&mut self, raw: bool) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let title = safe_title(&s.entry.title);
        let (pixels, w, h) = if raw {
            (s.gb.image().to_vec(), 160usize, 144usize)
        } else {
            // Fresh buffers: a screenshot is a rare, human-paced event, and
            // borrowing the render path's would tie it to the transmission
            // factor. Order is palette then upscale — the LCD grid must not be
            // quantised away (see `screen::frame`).
            let mut prep = FramePrep::new();
            let mapped = prep.map(s.gb.image(), self.palette).to_vec();
            let mut up = Upscaler::new(self.upscaler.algorithm(), self.upscale_factor);
            let (w, h) = up.output_size(160, 144);
            (up.apply(&mapped, 160, 144).to_vec(), w, h)
        };
        let look = if raw {
            "raw".to_string()
        } else {
            format!("{} {}", self.palette.name(), self.upscale_label())
        };

        let dir = screenshot_dir(&self.config);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.set_toast(format!("Screenshot failed: {} ({e})", dir.display()));
            return;
        }
        let path = unique_path(&dir, &format!("{title}-{}", now_stamp()));

        // Reuse the `image` crate the upscale gallery and the legacy render
        // path already use rather than adding an encoder. (`termimage`'s PNG
        // writer is deliberately *uncompressed* — right for a 60 Hz wire,
        // wrong for a file someone keeps.)
        let ok = image::RgbaImage::from_raw(w as u32, h as u32, pixels)
            .ok_or_else(|| "frame buffer does not match its dimensions".to_string())
            .and_then(|img| img.save(&path).map_err(|e| e.to_string()));
        match ok {
            Ok(()) => {
                self.set_toast(format!("Saved {} ({w}x{h} {look})", path.display()))
            }
            Err(e) => self.set_toast(format!("Screenshot failed: {e}")),
        }
    }

    /// Feed the raw video stream. `fresh` means an emulated frame was just
    /// produced; `false` is the idle tick that holds the stream's wall-clock
    /// rate while the game is paused or sitting under a modal overlay.
    ///
    /// Costs one `Option` test when video-out is off, which is the default —
    /// the clock is not even read.
    fn video_tick(&mut self, fresh: bool) {
        if !self.video.is_enabled() {
            // The game feed is off, but a held destination is still a live
            // camera and must keep receiving *something* — a consumer that stops
            // getting frames eventually drops the device, which is the failure
            // holding it was meant to prevent. `VideoOut::idle` rate-limits this
            // to [`video::IDLE_PERIOD`] itself.
            if self.video.is_idle() {
                self.video.idle(Instant::now());
            }
            return;
        }
        let now = Instant::now();
        if !fresh {
            self.video.idle(now);
            return;
        }
        let palette = self.palette;
        let source = self.video.spec().source;
        if source == VideoSource::Display {
            // Follow the live look: `[` (palette), `]`/`}` (upscaler) change
            // what the player sees, so they change what the stream carries.
            // A mid-stream size change is letterboxed into the stream's fixed
            // geometry rather than breaking the consumer — see `crate::video`.
            let algo = self.upscaler.algorithm();
            if self.video_upscaler.algorithm() != algo {
                self.video_upscaler.set_algorithm(algo);
            }
            if self.video_upscaler.factor() != self.upscale_factor {
                self.video_upscaler.set_factor(self.upscale_factor);
            }
        }
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let raw = s.gb.image();
        match source {
            VideoSource::Native => {
                self.video.frame(raw, video::SRC_W, video::SRC_H, now);
            }
            VideoSource::Display => {
                let mapped = self.video_prep.map(raw, palette);
                let (w, h) = self.video_upscaler.output_size(160, 144);
                let scaled = self.video_upscaler.apply(mapped, 160, 144);
                self.video.frame(scaled, w as u32, h as u32, now);
            }
        }
    }

    /// Write the game picture to the terminal, after `ratatui` has flushed the
    /// text frame.
    ///
    /// Writing it here rather than through a cell symbol is what makes an
    /// unchanged frame cost literally nothing, and what stops an overlay from
    /// freezing the picture. See [`crate::termimage`] for both failure modes.
    /// Returns whether a **picture** actually went out. A deduplicated frame
    /// writes nothing and therefore costs the terminal nothing — it must not be
    /// counted as an issued frame, and it is useless to fence behind. The bell
    /// below does not count either: it is one byte and no image work.
    fn flush_image(&mut self) -> io::Result<bool> {
        // The shiny bell rides out with the same flush: it is raised on an
        // emulated frame but must not be written mid-`ratatui` draw.
        if std::mem::take(&mut self.pending_bell) {
            use std::io::Write as _;
            let mut out = io::stdout().lock();
            out.write_all(b"\x07")?;
            out.flush()?;
        }
        // Music mode's cover is a picture of its own, in its own rectangle
        // with its own image id, and it is flushed by the same rule: after the
        // text frame, never into a cell symbol. See `crate::termimage`.
        #[cfg(feature = "music")]
        if let Some(m) = self.music.as_mut() {
            if !m.pending_wire.is_empty() {
                use std::io::Write as _;
                let mut out = io::stdout().lock();
                out.write_all(m.pending_wire.as_bytes())?;
                out.flush()?;
                m.pending_wire.clear();
            }
        }
        let Some(s) = self.session.as_mut() else {
            return Ok(false);
        };
        if s.pending_wire.is_empty() {
            return Ok(false);
        }
        use std::io::Write as _;
        let mut out = io::stdout().lock();
        out.write_all(s.pending_wire.as_bytes())?;
        out.flush()?;
        s.pending_wire.clear();
        s.frames_issued += 1;
        Ok(true)
    }

    /// Publish one telemetry record for the frame just drawn. Costs a single
    /// `Option` test when telemetry is off, which is the default.
    fn emit_telemetry(&mut self, draw_cost: Duration) {
        if !self.telemetry.is_enabled() {
            return;
        }
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let ctx = Context {
            rom_title: s.entry.title.clone(),
            mbc: s.entry.mbc.to_string(),
            frontend: "terminal".to_string(),
            protocol: protocol_name(self.picker.protocol_type()).to_string(),
            palette: self.palette.name().to_string(),
            upscale: self.upscaler.algorithm().name().to_string(),
            upscale_factor: self.upscaler.factor(),
            scale: s.scale,
            speed: SPEED_LADDER[self.speed].label.to_string(),
            term_cols: 0,
            term_rows: 0,
            display_cols: s.disp_cells.0,
            display_rows: s.disp_cells.1,
            tx_w: s.tx_px.0,
            tx_h: s.tx_px.1,
            display_px_w: s.disp_px.0,
            display_px_h: s.disp_px.1,
        };
        let sample = FrameSample {
            frame: s.emu_frames,
            fps: s.fps,
            emulated_fps: s.emu_fps,
            speed_pct: s.emu_fps / DMG_HZ * 100.0,
            frame_ms: telemetry::ms(draw_cost),
            emulate_ms: s.t_emulate,
            palette_ms: s.t_palette,
            upscale_ms: s.t_upscale,
            encode_ms: s.t_encode,
            write_ms: s.t_write,
            frames_skipped: s.skipped_since_display,
            frames_identical: s.frames_identical,
            frames_dropped: s.frames_dropped,
            tx_bytes: s.tx_bytes,
            frames_issued: s.frames_issued,
            terminal_ms: self.termcost.per_frame().map(|d| d.as_secs_f32() * 1000.0),
            terminal_ceiling_fps: self.termcost.ceiling_fps(),
            audio: s.audio_sample(),
            video: self.video.is_enabled().then(|| {
                let v = self.video.stats();
                VideoSample {
                    frames: v.frames,
                    duplicated: v.duplicated,
                    dropped: v.dropped,
                    bytes: v.bytes,
                    ms: v.last_ms,
                }
            }),
            // The AI agent's state, so an operator can watch a live stream's
            // health without reading the commentary panel — and so the claim
            // that a thinking agent costs the emulator nothing is checkable
            // from the same record that carries `speed_pct`.
            agent: Self::agent_sample(s),
        };
        self.telemetry.frame(&ctx, &sample);
    }

    /// The agent plugin's telemetry, or `None` when it is not attached — and
    /// likewise `None` in a build without the plugin, which is the same record
    /// a run with no agent produces. The schema does not change with the
    /// build; only the field's occupancy does.
    #[cfg(feature = "plugin-agent")]
    fn agent_sample(s: &Session) -> Option<telemetry::AgentSample> {
        s.gb.plugin::<terminalgb_plugin_agent::AgentPlugin>()
            .map(|p| {
                let g = p.governor();
                telemetry::AgentSample {
                    phase: match p.phase() {
                        terminalgb_plugin_agent::PhaseView::Settling => "settling",
                        terminalgb_plugin_agent::PhaseView::Thinking => "thinking",
                        terminalgb_plugin_agent::PhaseView::Executing => "executing",
                        terminalgb_plugin_agent::PhaseView::Stopped => "stopped",
                    },
                    decisions: p.decisions(),
                    calls: g.calls(),
                    failures: g.failures(),
                    tokens: g.tokens(),
                    usd: g.usd(),
                    budget_used: g.used_fraction(),
                    halted: g.halted().map(|h| h.label()).unwrap_or(""),
                }
            })
    }

    #[cfg(not(feature = "plugin-agent"))]
    fn agent_sample(_s: &Session) -> Option<telemetry::AgentSample> {
        None
    }

    fn release_expired(&mut self) -> bool {
        self.release_expired_at(Instant::now())
    }

    /// Auto-release every button whose hold window has run out.
    ///
    /// The window is [`KEY_HOLD`] under the kitty keyboard protocol (a safety
    /// net behind real release events) and [`Session::hold_window`] without it
    /// (the only thing that ever releases a button). Time is a parameter so the
    /// fallback can be tested over a virtual clock rather than by sleeping.
    fn release_expired_at(&mut self, now: Instant) -> bool {
        let mode = self.input;
        let mut changed = false;
        if let Some(s) = self.session.as_mut() {
            for idx in 0..8 {
                let window = s.hold_window(mode, idx);
                if let Some(t) = s.held[idx] {
                    if now.duration_since(t) >= window {
                        s.gb.keyup(key_from_index(idx));
                        s.held[idx] = None;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    // ---- input ----

    fn handle_key(&mut self, key: KeyEvent) {
        // Overlays intercept everything first, and every overlay is escapable.
        match self.overlay {
            Overlay::Help => {
                if key.kind == KeyEventKind::Press {
                    self.overlay = Overlay::None;
                }
                return;
            }
            Overlay::Quit => {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                            self.should_quit = true
                        }
                        _ => self.overlay = Overlay::None,
                    }
                }
                return;
            }
            Overlay::None => {}
        }

        match self.screen {
            Screen::Picker => self.handle_picker_key(key),
            Screen::Playing => self.handle_game_key(key),
            #[cfg(feature = "music")]
            Screen::Shelf => self.handle_shelf_key(key),
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        self.message = None;

        // While filtering, printable keys edit the query.
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filtering = false;
                    self.filter.clear();
                    self.recompute_visible();
                }
                KeyCode::Enter => {
                    self.filtering = false;
                    self.launch_selected();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.recompute_visible();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.recompute_visible();
                }
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Down => self.move_selection(1),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('r') => self.rescan(),
            // Music mode. Upper case because `m` is the mute key everywhere
            // else in this frontend and a picker that mutes on one screen and
            // changes screen on another would be a trap.
            #[cfg(feature = "music")]
            KeyCode::Char('M') => self.enter_music(),
            // Resume the most-recently-played ROM directly, even if it lives
            // outside the current `rom_dir` (so it isn't in the list).
            KeyCode::Char('.') => self.resume_last(),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.visible.len().saturating_sub(1)
            }
            KeyCode::Enter => self.launch_selected(),
            _ => {}
        }
    }

    /// The title of the most-recently-played ROM, if any (for the picker hint).
    fn last_played_title(&self) -> Option<String> {
        let path = self.recent.paths.first()?;
        Some(
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        )
    }

    /// Launch the most-recently-played ROM straight away.
    fn resume_last(&mut self) {
        if let Some(path) = self.recent.paths.first().cloned() {
            self.start_game(&path);
        } else {
            self.message = Some("no recently-played ROM yet".to_string());
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let len = self.visible.len() as isize;
        let mut sel = self.selected as isize + delta;
        if sel < 0 {
            sel = 0;
        }
        if sel >= len {
            sel = len - 1;
        }
        self.selected = sel as usize;
    }

    fn launch_selected(&mut self) {
        if let Some(&ei) = self.visible.get(self.selected) {
            let path = self.entries[ei].path.clone();
            self.start_game(&path);
        }
    }

    fn handle_game_key(&mut self, key: KeyEvent) {
        self.handle_game_key_at(key, Instant::now());
    }

    fn handle_game_key_at(&mut self, key: KeyEvent, now: Instant) {
        let press = key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat;
        let release = key.kind == KeyEventKind::Release;

        // In basic mode the terminal's auto-repeat is indistinguishable from a
        // real second press, so a one-shot control key held down would fire ~30
        // times a second — palette strobing, the video stream flapping on and
        // off. Suppress a same-key event that lands inside the lockout window.
        // Enhanced mode is untouched: there `Repeat` is a distinct kind and the
        // existing behaviour is deliberate.
        let control_ok = if press && self.input == InputMode::Basic {
            let repeated = matches!(
                self.last_control,
                Some((code, t))
                    if code == key.code
                        && now.saturating_duration_since(t) < CONTROL_REPEAT_LOCKOUT
            );
            self.last_control = Some((key.code, now));
            !repeated
        } else {
            true
        };

        // Control keys act on press only.
        if press && control_ok {
            match key.code {
                KeyCode::Char('q') => {
                    self.overlay = Overlay::Quit;
                    return;
                }
                KeyCode::Char('?') => {
                    self.overlay = Overlay::Help;
                    return;
                }
                KeyCode::Char('p') => {
                    if let Some(s) = self.session.as_mut() {
                        s.paused = !s.paused;
                    }
                    return;
                }
                // `W` for "which console" — `L` is already the way back to
                // the picker, and the four game buttons default to a/s/x/z.
                KeyCode::Char('W') => {
                    self.link_key();
                    return;
                }
                KeyCode::Char('R') => {
                    self.reset_game();
                    return;
                }
                KeyCode::Char('L') => {
                    self.back_to_picker();
                    return;
                }
                KeyCode::Char('f') => {
                    self.fullscreen = !self.fullscreen;
                    return;
                }
                KeyCode::Char('[') => {
                    self.palette = self.palette.next();
                    return;
                }
                // Upscaler: `]` cycles the algorithm, `}` the scale factor.
                KeyCode::Char(']') => {
                    self.upscaler.cycle_algorithm();
                    self.upscale_factor =
                        self.upscaler.algorithm().clamp_factor(self.upscale_factor);
                    let label = self.upscale_label();
                    self.set_toast(format!("Upscale {label}"));
                    return;
                }
                KeyCode::Char('}') => {
                    self.upscale_factor =
                        self.upscaler.algorithm().next_factor(self.upscale_factor);
                    let label = self.upscale_label();
                    self.set_toast(format!("Upscale {label}"));
                    return;
                }
                // Video stream on/off. `v` for video — free, and the one key
                // nobody has to look up. `V` records a file instead, so the two
                // intents are separately reachable however `video_out` is set
                // (see `App::toggle_recording`).
                KeyCode::Char('v') => {
                    self.toggle_video();
                    return;
                }
                KeyCode::Char('V') => {
                    self.toggle_recording();
                    return;
                }
                // Screenshot: `c` captures what you see (palette + upscaler),
                // `C` the raw 160x144 framebuffer. F2 is the emulator-world
                // convention and costs one arm. Chosen over `s`/`p`/`x`: those
                // are game buttons or already bound.
                KeyCode::Char('c') | KeyCode::F(2) => {
                    self.screenshot(false);
                    return;
                }
                KeyCode::Char('C') => {
                    self.screenshot(true);
                    return;
                }
                // Stats overlay: off by default, does not pause the game.
                KeyCode::Char('t') if key.kind == KeyEventKind::Press => {
                    self.stats = !self.stats;
                    return;
                }
                // Live trainer panel. `T` is deliberately shifted: the game
                // buttons are configurable lowercase letters, so an unshifted
                // key could collide with somebody's A/B binding.
                KeyCode::Char('T') => {
                    self.toggle_trainer();
                    return;
                }
                // Battle control (Pokémon Red/Blue, `plugins = gen1-battle`).
                // `B` fires the selected preset, `N` chooses the next one.
                // Both are shifted for the same reason `T` is: the game
                // buttons are configurable lowercase letters, so an unshifted
                // key could be somebody's A/B binding.
                KeyCode::Char('B') => {
                    self.fire_battle();
                    return;
                }
                KeyCode::Char('N') => {
                    self.next_battle_preset();
                    return;
                }
                // The AI agent's commentary panel, and the switch that hands
                // the controller back to a human. Both shifted, for the same
                // reason `T` is: the unshifted letters are game buttons.
                KeyCode::Char('A') => {
                    self.agent_panel = !self.agent_panel;
                    let on = self.agent_panel;
                    self.set_toast(
                        if on {
                            "Commentary panel on"
                        } else {
                            "Commentary panel off"
                        }
                        .to_string(),
                    );
                    return;
                }
                KeyCode::Char('G') => {
                    self.toggle_agent();
                    return;
                }
                // Detail view and party selection only bind while the panel is
                // up, so they stay free as game buttons the rest of the time.
                KeyCode::Char('d') if self.trainer => {
                    self.trainer_detail = !self.trainer_detail;
                    return;
                }
                KeyCode::Tab if self.trainer => {
                    self.cycle_trainer_selection(1);
                    return;
                }
                KeyCode::BackTab if self.trainer => {
                    self.cycle_trainer_selection(-1);
                    return;
                }
                // Speed control: `+`/`=` faster, `-`/`_` slower.
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.bump_speed(true);
                    return;
                }
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    self.bump_speed(false);
                    return;
                }
                // Playback volume: `,`/`<` quieter, `.`/`>` louder, `m` mute.
                // The comma/full-stop pair is the media-player convention and
                // both are free here (`.` is the picker's resume key, a
                // different screen); `m` is free of the *default* game buttons
                // (a/s/x/z) and is what a person reaches for to silence
                // something. This is host playback only — see
                // `crate::sound::output_volume` for why it is not NR50.
                //
                // All three yield to a user who has bound that character as a
                // game button: `key_a = m` must still mean A. The player's own
                // configuration outranks ours, and the guard costs one lookup
                // on a key press.
                KeyCode::Char(',') | KeyCode::Char('<')
                    if !self.is_game_button(&key.code) =>
                {
                    self.bump_volume(false);
                    return;
                }
                KeyCode::Char('.') | KeyCode::Char('>')
                    if !self.is_game_button(&key.code) =>
                {
                    self.bump_volume(true);
                    return;
                }
                KeyCode::Char('m') if !self.is_game_button(&key.code) => {
                    self.toggle_mute();
                    return;
                }
                // Save-slot select.
                KeyCode::Char(c @ '1'..='9') => {
                    let n = c as u32 - '0' as u32;
                    if (1..=NUM_SLOTS).contains(&n) {
                        self.slot = n;
                        self.set_toast(format!("Slot {n}"));
                    }
                    return;
                }
                // Save/load the active slot (emulator-standard quicksave keys).
                KeyCode::F(5) if key.kind == KeyEventKind::Press => {
                    self.save_slot();
                    return;
                }
                KeyCode::F(8) if key.kind == KeyEventKind::Press => {
                    self.load_slot();
                    return;
                }
                KeyCode::Esc => {
                    if self.fullscreen {
                        self.fullscreen = false;
                    }
                    return;
                }
                _ => {}
            }
        }

        // Game-button keys (movement + configurable A/B/Start/Select). These
        // deliberately ignore `control_ok` — a repeat of a *button* is exactly
        // the evidence that it is still held, which is what basic mode runs on.
        if let Some(button) = self.map_button(&key.code) {
            if press {
                self.press_button_at(button, now);
            } else if release {
                self.release_button(button);
            }
        }
    }

    /// Start the selected battle preset. One keypress, as asked for.
    ///
    /// Nothing happens unless the user configured `plugins = gen1-battle`, and
    /// the plugin then still waits for the game to be somewhere a battle can
    /// start — so this is "arm it", not "force it". Both outcomes are toasted,
    /// because a control that silently does nothing is worse than no control.
    #[cfg(feature = "plugin-battle-control")]
    fn fire_battle(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(p) =
            s.gb.plugin_mut::<terminalgb_plugin_battle_control::BattlePlugin>()
        else {
            self.set_toast(
                "no battle plugin — add `plugins = gen1-battle` to config.toml",
            );
            return;
        };
        let name = p.selected().map(|x| x.name.clone());
        let msg = match p.arm_selected() {
            Ok(()) => match name {
                Some(n) => format!("Battle: {n}"),
                None => "Battle armed".to_string(),
            },
            Err(e) => format!("Battle: {e}"),
        };
        self.set_toast(msg);
    }

    #[cfg(not(feature = "plugin-battle-control"))]
    fn fire_battle(&mut self) {
        self.set_toast(
            "battle control is not built in — rebuild with --features plugin-battle-control",
        );
    }

    /// Cycle which preset `B` will fire.
    #[cfg(feature = "plugin-battle-control")]
    fn next_battle_preset(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(p) =
            s.gb.plugin_mut::<terminalgb_plugin_battle_control::BattlePlugin>()
        else {
            self.set_toast(
                "no battle plugin — add `plugins = gen1-battle` to config.toml",
            );
            return;
        };
        let msg = match p.select_next() {
            Some(x) => format!("Battle: {} — {}", x.name, x.summary()),
            None => "no battle presets".to_string(),
        };
        self.set_toast(msg);
    }

    #[cfg(not(feature = "plugin-battle-control"))]
    fn next_battle_preset(&mut self) {
        self.set_toast(
            "battle control is not built in — rebuild with --features plugin-battle-control",
        );
    }

    fn map_button(&self, code: &KeyCode) -> Option<KeypadKey> {
        match code {
            KeyCode::Up => Some(KeypadKey::Up),
            KeyCode::Down => Some(KeypadKey::Down),
            KeyCode::Left => Some(KeypadKey::Left),
            KeyCode::Right => Some(KeypadKey::Right),
            KeyCode::Char(c) => {
                let lc = c.to_ascii_lowercase();
                let k = &self.config.keys;
                if lc == k.a.to_ascii_lowercase() {
                    Some(KeypadKey::A)
                } else if lc == k.b.to_ascii_lowercase() {
                    Some(KeypadKey::B)
                } else if lc == k.start.to_ascii_lowercase() {
                    Some(KeypadKey::Start)
                } else if lc == k.select.to_ascii_lowercase() {
                    Some(KeypadKey::Select)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Press a button and refresh its hold window, learning this terminal's
    /// auto-repeat rate on the way (see [`Session::repeat_gap`]).
    fn press_button_at(&mut self, button: KeypadKey, now: Instant) {
        if let Some(s) = self.session.as_mut() {
            let idx = key_index(button);
            if let Some(prev) = s.last_press[idx] {
                let gap = now.saturating_duration_since(prev);
                // Above the cap this is the auto-repeat *delay* or a human
                // pressing twice, neither of which says anything about the
                // rate. Below it, keep the shortest — auto-repeat is the
                // fastest thing that can produce two presses of one key.
                if !gap.is_zero() && gap <= KEY_REPEAT_LEARN_MAX {
                    s.repeat_gap[idx] =
                        Some(s.repeat_gap[idx].map_or(gap, |g| g.min(gap)));
                }
            }
            s.last_press[idx] = Some(now);
            s.gb.keydown(button);
            s.held[idx] = Some(now);
        }
    }

    fn release_button(&mut self, button: KeypadKey) {
        if let Some(s) = self.session.as_mut() {
            s.gb.keyup(button);
            s.held[key_index(button)] = None;
        }
    }

    fn reset_game(&mut self) {
        if let Some(s) = self.session.as_ref() {
            let path = s.entry.path.clone();
            self.start_game(&path);
        }
    }

    // ---- speed / save-state / stats ----

    fn bump_speed(&mut self, up: bool) {
        self.speed = if up {
            (self.speed + 1).min(SPEED_LADDER.len() - 1)
        } else {
            self.speed.saturating_sub(1)
        };
        // The audio player paces the emulator against the audio clock at 1x
        // (high-watermark wait). Any non-1x rung must switch it to
        // drop-on-overflow, or the wait would clamp fast-forward back to
        // real time (and slow-motion is fine either way — the queue just
        // drains and re-primes).
        if let Some(s) = self.session.as_mut() {
            s.gb.set_audio_fast_forward(self.speed != SPEED_NORMAL);
        }
        self.set_toast(format!("Speed {}", SPEED_LADDER[self.speed].label));
    }

    /// Move playback volume one rung and remember it.
    ///
    /// Host-side only: the emulated machine is untouched at every level, so no
    /// accuracy suite and no audio baseline can see this happen. See
    /// [`crate::sound::output_volume`].
    fn bump_volume(&mut self, up: bool) {
        output_volume::step(up);
        self.persist_volume();
        self.set_toast(format!("Volume {}", output_volume::label()));
    }

    fn toggle_mute(&mut self) {
        let muted = output_volume::toggle_mute();
        self.persist_volume();
        self.set_toast(if muted {
            "Muted (m to unmute)".to_string()
        } else {
            format!("Volume {}", output_volume::label())
        });
    }

    /// Write the live volume to the state file, so tonight's setting is still
    /// tonight's setting tomorrow. Best-effort and tiny (a few dozen bytes on a
    /// key press); it never touches the frame path.
    fn persist_volume(&self) {
        VolumeState::save(
            output_volume::level(),
            output_volume::muted(),
            self.config.volume,
        );
    }

    /// Is this key one of the player's configured game buttons? Control keys
    /// are matched before game buttons, so anything bound to a letter has to
    /// check first or it silently steals somebody's A button.
    fn is_game_button(&self, code: &KeyCode) -> bool {
        self.map_button(code).is_some()
    }

    /// Switch the trainer panel on or off, telling the plugin so that a hidden
    /// panel costs nothing.
    #[cfg(feature = "plugin-trainer")]
    fn toggle_trainer(&mut self) {
        self.trainer = !self.trainer;
        let on = self.trainer;
        let mut msg = None;
        if let Some(s) = self.session.as_mut() {
            if let Some(p) = s.gb.plugin_mut::<TrainerPlugin>() {
                p.set_active(on);
                if on {
                    msg = p.refusal();
                }
            }
        }
        match msg {
            // An unsupported cartridge says so once, rather than showing an
            // empty panel the user has to guess about.
            Some(why) => self.set_toast(why),
            None if on => self.set_toast("Trainer view · Tab select · d detail"),
            None => {}
        }
    }

    #[cfg(not(feature = "plugin-trainer"))]
    /// The trainer view is not built in. Say so rather than doing nothing:
    /// a key that silently fails reads as a bug.
    fn toggle_trainer(&mut self) {
        self.set_toast(
            "trainer view is not built in — rebuild with --features plugin-trainer",
        );
    }

    /// Hand the controller between the agent and the human (`G`).
    ///
    /// The game keeps running either way — switching the agent off does not
    /// pause anything, it just stops something else pressing the buttons.
    #[cfg(feature = "plugin-agent")]
    fn toggle_agent(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(p) = s.gb.plugin_mut::<terminalgb_plugin_agent::AgentPlugin>() else {
            self.set_toast("No agent is attached".to_string());
            return;
        };
        let on = !p.is_enabled();
        p.set_enabled(on);
        self.set_toast(
            if on {
                "Agent is playing"
            } else {
                "Agent stopped — you have the controller"
            }
            .to_string(),
        );
    }

    #[cfg(not(feature = "plugin-agent"))]
    fn toggle_agent(&mut self) {
        self.set_toast("agent is not built in — rebuild with --features plugin-agent");
    }

    /// Walk the focus ring, which during a battle **starts at the opponent**
    /// and then runs through the party — so the opponent's full stat block is
    /// one `Tab` + `d` away, and the team you would switch to is the next few.
    /// Before this the ring was the party alone and the opponent's numbers were
    /// unreachable, which is most of what a tracker is for.
    #[cfg(feature = "plugin-trainer")]
    fn cycle_trainer_selection(&mut self, delta: isize) {
        let n = self
            .session
            .as_ref()
            .and_then(|s| s.gb.plugin::<TrainerPlugin>())
            .map(|p| tv::focus_ring(p.state()).len())
            .unwrap_or(0);
        if n == 0 {
            return;
        }
        let cur = self.trainer_sel.min(n - 1) as isize;
        self.trainer_sel = (cur + delta).rem_euclid(n as isize) as usize;
    }

    #[cfg(not(feature = "plugin-trainer"))]
    fn cycle_trainer_selection(&mut self, _delta: isize) {}

    fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    /// The toast text if it is still fresh (shown for ~2.5 s).
    fn toast_text(&self) -> Option<&str> {
        match &self.toast {
            Some((msg, when)) if when.elapsed() < Duration::from_millis(2500) => {
                Some(msg.as_str())
            }
            _ => None,
        }
    }

    /// Save the running machine to the active slot (a file the terminal owns;
    /// the core just hands us bytes).
    fn save_slot(&mut self) {
        let slot = self.slot;
        let result = self.session.as_ref().map(|s| {
            let path = state_path(&s.gb.rom_title(), slot);
            let bytes = s.gb.save_state();
            (bytes.len(), std::fs::write(&path, &bytes))
        });
        match result {
            Some((len, Ok(()))) => {
                self.set_toast(format!("★ Saved slot {slot} ({} KB)", len / 1024))
            }
            Some((_, Err(e))) => self.set_toast(format!("Save failed: {e}")),
            None => {}
        }
    }

    /// Restore the active slot into the running machine, then rebuild the
    /// display image so the restored frame shows immediately.
    fn load_slot(&mut self) {
        let slot = self.slot;
        let outcome = if let Some(s) = self.session.as_mut() {
            let path = state_path(&s.gb.rom_title(), slot);
            match std::fs::read(&path) {
                Ok(bytes) => match s.gb.load_state(&bytes) {
                    // The next draw rebuilds the picture from `gb.image()`, so
                    // the restored frame shows immediately with no extra work.
                    Ok(()) => Ok(()),
                    Err(e) => Err(format!("Load failed: {e}")),
                },
                Err(_) => Err(format!("Slot {slot} empty")),
            }
        } else {
            return;
        };
        match outcome {
            Ok(()) => self.set_toast(format!("↺ Loaded slot {slot}")),
            Err(msg) => self.set_toast(msg),
        }
    }

    // ---- rendering ----

    fn render(&mut self, f: &mut Frame<'_>) {
        match self.screen {
            Screen::Picker => self.render_picker(f),
            Screen::Playing => self.render_game(f),
            #[cfg(feature = "music")]
            Screen::Shelf => self.render_shelf(f),
        }
        match self.overlay {
            Overlay::Help => self.render_help(f),
            Overlay::Quit => render_quit(f),
            Overlay::None => {}
        }
    }

    fn render_picker(&mut self, f: &mut Frame<'_>) {
        let area = f.area();
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),                                  // header
                Constraint::Min(0),                                     // list
                Constraint::Length(if self.filtering { 1 } else { 0 }), // filter
                Constraint::Length(1),                                  // footer
            ])
            .split(area);

        let header = format!(
            " TerminalGB · pick a ROM — {}",
            self.config.rom_dir.display()
        );
        f.render_widget(
            Paragraph::new(header).style(Style::default().add_modifier(Modifier::BOLD)),
            rows[0],
        );

        let list_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" ROMs ({}) ", self.visible.len()));

        if self.entries.is_empty() {
            let msg = self.message.clone().unwrap_or_else(|| {
                format!(
                    "No .gb/.gbc ROMs found in {}.\n\nSet `rom_dir` in {} or pass a ROM path on the command line.",
                    self.config.rom_dir.display(),
                    crate::screen::config::config_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "the config file".into()),
                )
            });
            f.render_widget(
                Paragraph::new(msg)
                    .wrap(Wrap { trim: true })
                    .block(list_block),
                rows[1],
            );
        } else {
            let items: Vec<ListItem> = self
                .visible
                .iter()
                .map(|&ei| {
                    let e = &self.entries[ei];
                    let star = if self.recent_flags[ei] { "★ " } else { "  " };
                    let line = format!(
                        "{star}{title:<20}  {mbc:<22}  {size} KiB",
                        title = truncate(&e.title, 20),
                        mbc = e.mbc,
                        size = e.size / 1024,
                    );
                    ListItem::new(line)
                })
                .collect();
            let list = List::new(items).block(list_block).highlight_style(
                Style::default()
                    .bg(Color::Indexed(24))
                    .add_modifier(Modifier::BOLD),
            );
            let mut state = ListState::default();
            if !self.visible.is_empty() {
                state.select(Some(self.selected.min(self.visible.len() - 1)));
            }
            f.render_stateful_widget(list, rows[1], &mut state);
        }

        if self.filtering {
            f.render_widget(
                Paragraph::new(format!("/{}", self.filter))
                    .style(Style::default().fg(Color::Yellow)),
                rows[2],
            );
        }

        let footer = if let Some(msg) = &self.message {
            Span::styled(format!(" {msg}"), Style::default().fg(Color::Red))
        } else {
            let base = " ↑/↓ move · enter play · / filter · r rescan · ? help · q quit";
            let hint = match self.last_played_title() {
                Some(title) => format!("{base} · . resume {title}"),
                None => base.to_string(),
            };
            Span::styled(hint, Style::default().fg(Color::DarkGray))
        };
        f.render_widget(Paragraph::new(Line::from(footer)), rows[3]);
    }

    fn render_game(&mut self, f: &mut Frame<'_>) {
        let area = f.area();
        self.stats_rect = None;
        self.trainer_rect = None;
        self.sprite_rect = None;
        self.agent_rect = None;

        if self.fullscreen {
            // Even in fullscreen the stats panel and the toast take their own
            // space rather than sitting on top of the picture. A terminal
            // graphics image owns whole cells: text drawn into a cell the image
            // covers *replaces* that part of the image, and for Kitty's
            // placeholder placement it stops the picture updating altogether.
            // See `render_stats`.
            let mut rest = area;
            if self.toast_text().is_some() && rest.height > 1 {
                rest.height -= 1;
                self.render_toast(f, Rect::new(area.x, area.bottom() - 1, area.width, 1));
            }
            // The encounter verdict takes the top row, above everything. It is
            // the one thing here that is time-critical: a Ball is about to be
            // spent or not spent.
            if self.banner_state() != 0 && rest.height > 1 {
                self.render_encounter_banner(f, Rect::new(rest.x, rest.y, rest.width, 1));
                rest = Rect::new(rest.x, rest.y + 1, rest.width, rest.height - 1);
            }
            // The commentary panel takes whole rows off the bottom. It must
            // never be drawn *into* the picture's cells — see agent_view.
            let mut agent_panel = None;
            if self.agent_panel {
                let (game, panel) = av::split(rest, av::PANEL_H);
                if let Some(p) = panel {
                    rest = game;
                    agent_panel = Some(p);
                }
            }
            let mut trainer_panel = None;
            let panel_w = trainer_panel_width(rest.width);
            if self.trainer && panel_w >= tv::PANEL_MIN_W {
                trainer_panel = Some(Rect::new(
                    rest.right() - panel_w,
                    rest.y,
                    panel_w,
                    rest.height,
                ));
                rest = Rect::new(rest.x, rest.y, rest.width - panel_w, rest.height);
            }
            if self.stats && rest.width > STATS_W + 8 {
                let panel = Rect::new(rest.x, rest.y, STATS_W, rest.height);
                rest = Rect::new(
                    rest.x + STATS_W,
                    rest.y,
                    rest.width - STATS_W,
                    rest.height,
                );
                self.render_stats(f, panel);
            }
            self.render_display(f, rest);
            // After the picture: `render_display` clears the pending image
            // sequence for the frame, and the sprite's sequence has to survive
            // into the same flush.
            if let Some(panel) = trainer_panel {
                self.render_trainer(f, panel);
            }
            if let Some(panel) = agent_panel {
                self.render_agent(f, panel);
            }
            return;
        }

        // The encounter banner is a row of its own, directly under the title —
        // never over the picture (a terminal image owns whole cells) and never
        // folded into the footer, which the toast already owns.
        let banner_h = u16::from(self.banner_state() != 0);
        // The bottom row is taken **only while a toast is up**. The controls
        // that used to live there are on the top bar now, which is what frees
        // it — so the tall narrow split this was reported from gains a row
        // rather than spending one on the bar.
        let toast_h = u16::from(self.toast_text().is_some());
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),        // the bar
                Constraint::Length(banner_h), // wild-encounter verdict
                Constraint::Min(0),           // body
                Constraint::Length(toast_h),  // toast, when there is one
            ])
            .split(area);
        if banner_h > 0 {
            self.render_encounter_banner(f, rows[1]);
        }
        self.render_top_bar(f, rows[0]);

        // With the trainer panel up the right-hand column becomes the panel:
        // the Status sidebar is reference information, the party is what is
        // being watched, and splitting the column three ways would make both
        // unreadable.
        // Commentary sits across the bottom of the body, under both the
        // picture and the sidebar — the captain asked for "a text window on
        // the bottom", and full width is also what gives it enough columns to
        // stay readable at the large font a stream needs. It is taken off
        // **before** the picture/panel block is composed, so the block is
        // centred in what is left rather than being pushed off the bottom.
        let (body_area, agent_panel) = if self.agent_panel {
            av::split(rows[2], av::PANEL_H)
        } else {
            (rows[2], None)
        };
        // **The picture and the panel are ONE block.** See `compose_block`: the
        // picture column is exactly as wide as the picture, the panel butts
        // straight up against it, the two share a vertical extent, and the
        // whole thing is centred in the body. Anything else and the leftover
        // space lands *between* them, which is what made the two halves read as
        // unrelated.
        let panel_w = if self.trainer {
            trainer_panel_width(body_area.width)
        } else {
            0
        };
        let block = (self.trainer && panel_w >= tv::PANEL_MIN_W).then(|| {
            compose_block(
                body_area,
                panel_w,
                self.trainer_panel_rows(panel_w),
                self.picker.font_size(),
            )
        });
        // Without the trainer panel nothing changes: the Status sidebar keeps
        // its own fixed column and the picture keeps the rest.
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(30)])
            .split(body_area);
        let (game, trainer_panel) = match block {
            Some((g, p)) => (g, Some(p)),
            None => (body[0], None),
        };

        // The stats panel lives in the sidebar, above Status — never over the
        // picture.
        if self.trainer {
            // drawn after render_display, below
        } else if self.stats {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(STATS_H), Constraint::Min(0)])
                .split(body[1]);
            self.render_stats(f, split[0]);
            self.render_sidebar(f, split[1]);
        } else {
            self.render_sidebar(f, body[1]);
        }
        self.render_display(f, game);
        if let Some(panel) = trainer_panel {
            self.render_trainer(f, panel);
        }
        if let Some(panel) = agent_panel {
            self.render_agent(f, panel);
        }

        // The toast gets the bottom row for as long as it is up — it must not
        // land on the picture (a terminal image owns whole cells).
        if toast_h > 0 {
            self.render_toast(f, rows[3]);
        }
        if let Some(panel) = agent_panel {
            self.render_agent(f, panel);
        }
    }

    /// Everything about the UI that can paint over a terminal image. When it
    /// changes, every image must be retransmitted even if its pixels did not,
    /// because the terminal's copy may have been partly overwritten.
    fn ui_signature(&self) -> UiSignature {
        (
            match self.overlay {
                Overlay::None => 0u8,
                Overlay::Help => 1,
                Overlay::Quit => 2,
            },
            self.fullscreen,
            self.stats,
            self.toast_text().is_some(),
            self.trainer,
            self.trainer_detail,
            // The encounter banner takes a row, which moves everything below it
            // — including the picture.
            self.banner_state(),
            self.agent_panel,
        )
    }

    /// The AI agent's commentary panel — the part that makes this a stream.
    ///
    /// Its own rectangle, never over the picture (see
    /// [`crate::screen::agent_view`]). It draws whatever the plugin has to
    /// say; with no agent attached it explains that, rather than being an
    /// empty box nobody can interpret.
    #[cfg(feature = "plugin-agent")]
    fn render_agent(&mut self, f: &mut Frame<'_>, area: Rect) {
        if area.width < av::MIN_W || area.height < av::MIN_H {
            return;
        }
        self.agent_rect = Some(area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta))
            .title(" Commentary ");
        let inner_h = area.height.saturating_sub(2);
        let inner_w = area.width.saturating_sub(2);

        let body: Vec<Line<'static>> = match self
            .session
            .as_ref()
            .and_then(|s| s.gb.plugin::<terminalgb_plugin_agent::AgentPlugin>())
        {
            Some(p) => match p.refusal() {
                Some(why) => vec![Line::from(Span::styled(
                    why,
                    Style::default().fg(Color::DarkGray),
                ))],
                None => av::lines(p.commentary(), inner_w, inner_h),
            },
            None => vec![Line::from(Span::styled(
                "No agent attached. Start with plugins = agent (or GB_PLUGINS=agent) \
                 and an agent_command to watch Claude play.",
                Style::default().fg(Color::DarkGray),
            ))],
        };
        f.render_widget(
            Paragraph::new(body)
                .wrap(ratatui::widgets::Wrap { trim: true })
                .block(block),
            area,
        );
    }

    #[cfg(not(feature = "plugin-agent"))]
    fn render_agent(&mut self, _f: &mut Frame<'_>, _area: Rect) {}

    /// How many rows the trainer panel would like, so `compose_block` can give
    /// the block a height that suits both halves.
    ///
    /// The body lines are composed here and again in `render_trainer`, which is
    /// a few dozen small allocations once per **displayed** frame — the price of
    /// the count being the same thing that is drawn rather than a second
    /// estimate that can drift from it.
    #[cfg(feature = "plugin-trainer")]
    fn trainer_panel_rows(&self, panel_w: u16) -> u16 {
        let Some(p) = self
            .session
            .as_ref()
            .and_then(|s| s.gb.plugin::<TrainerPlugin>())
        else {
            return 0;
        };
        // The refusal and waiting-for-a-save screens are prose in a box; a few
        // rows is all they want.
        if p.game().is_none() || !p.state().in_game {
            return 8;
        }
        let inner = panel_w.saturating_sub(2) as usize;
        let st = p.state();
        let focus = tv::focus_at(st, self.trainer_sel);
        let lines = tv::body_lines(st, focus, self.trainer_detail, inner).len() as u16;
        // The pictures are *asked for* here rather than decided: `layout_with_team`
        // still refuses either if the block turns out too short, and asking for
        // them cannot make the block shorter than the text needs.
        let sprite = u16::from(
            !self.trainer_detail
                && focus
                    .and_then(|fo| tv::focused_mon(st, fo))
                    .is_some_and(|m| m.species != 0),
        ) * tv::sprite_rows(panel_w.saturating_sub(2));
        let team = u16::from(
            !self.trainer_detail
                && !st.enemy_team.is_empty()
                && panel_w.saturating_sub(2) >= tv::TEAM_MIN_W,
        ) * tv::TEAM_ROWS;
        2 + tv::HEADER_ROWS + sprite + team + lines
    }

    #[cfg(not(feature = "plugin-trainer"))]
    fn trainer_panel_rows(&self, _panel_w: u16) -> u16 {
        0
    }

    /// Take the pending full-repaint request, if any. See [`App::full_redraw`].
    fn take_full_redraw(&mut self) -> bool {
        std::mem::replace(&mut self.full_redraw, false)
    }

    /// The bar across the top of the game screen: what is running, what is
    /// live, **what the plugins are doing**, and the keys in constant use.
    ///
    /// # Why it is here and not along the bottom
    ///
    /// The controls used to run along the last row, which is the row a terminal
    /// clips when the window's pixel height is not a whole number of cells —
    /// exactly the defect that was reported. Plugin state lived inside the
    /// trainer panel, competing for space with the party and the battle, which
    /// is the panel's actual job; and nothing on screen advertised that plugins
    /// existed at all, so the link partner had to be discovered by reading the
    /// documentation. Both belong where a person looks first.
    ///
    /// **It costs no rows.** It replaces the title row rather than adding to
    /// it, and the bottom row it replaces is now taken only while a toast is
    /// up — so in the tall narrow split this work was done for, the layout
    /// gains a row rather than spending one.
    ///
    /// # What it carries, and what it drops first
    ///
    /// Identity, then anything abnormal (paused, fast-forwarding, recording),
    /// then the plugins, then the keys. Each of those has its own ladder of
    /// shorter forms and shrinks through it before anything is dropped — see
    /// [`fit_bar`]. It is a **stream-facing** surface, so it names things the
    /// way a viewer would and never shows a path.
    ///
    /// The full key reference stays behind `?`; this is the subset in constant
    /// use.
    fn render_top_bar(&self, f: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let title = self
            .session
            .as_ref()
            .map(|s| s.entry.title.clone())
            .unwrap_or_else(|| "terminalgb".to_string());

        // Anything abnormal. Nothing here appears in the ordinary case, which
        // is the point: the bar should be quiet until something is true.
        let mut flags: Vec<String> = Vec::new();
        if self.session.as_ref().is_some_and(|s| s.paused) {
            flags.push("PAUSED".into());
        }
        if self.speed != SPEED_NORMAL {
            flags.push(SPEED_LADDER[self.speed].label.to_string());
        }
        let video = self.video_status();
        if !video.starts_with("off") {
            flags.push(video.split_whitespace().next().unwrap_or("REC").to_string());
        }
        // Silence has to be self-explaining: a muted emulator looks exactly
        // like a broken one. Only the abnormal states appear here — muted, or
        // turned right down — so the bar stays quiet at an ordinary level.
        if output_volume::muted() {
            flags.push("MUTED".into());
        } else if output_volume::level() == 0 {
            flags.push("VOL 0%".into());
        }

        let plugins = self
            .session
            .as_ref()
            .map(|s| s.gb.plugin_statuses())
            .unwrap_or_default();
        // The detail is bounded rather than shown in full: a plugin's own
        // summary can be a whole sentence (the trainer view reports the live
        // encounter), and the bar's job is *state at a glance*, not a second
        // copy of what the encounter banner already says a row below.
        let long: Vec<String> = plugins
            .iter()
            .map(|p| {
                if p.detail.is_empty() {
                    p.name.clone()
                } else {
                    format!("{} {}", p.name, truncate(&p.detail, 18))
                }
            })
            .collect();
        let names: Vec<String> = plugins.iter().map(|p| p.name.clone()).collect();

        let ladders = vec![
            vec![title.clone(), truncate(&title, 12)],
            vec![flags.join(" · ")],
            vec![
                long.join(" · "),
                names.join(" · "),
                match plugins.len() {
                    0 => String::new(),
                    1 => names.first().cloned().unwrap_or_default(),
                    n => format!("{n} plugins"),
                },
            ],
            vec![
                "? help · T trainer · A commentary · p pause · F5/F8 save · +/- speed · ,/. vol · q quit"
                    .into(),
                "? help · T trainer · A commentary · p pause · ,/. vol · q quit".into(),
                "? help · T trainer · p pause · ,/. vol · q quit".into(),
                "? help · T trainer · q quit".into(),
                "? help".into(),
            ],
        ];
        let fitted = fit_bar(&ladders, "  ", width);

        let dim = Style::default().fg(Color::DarkGray);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let push = |text: &str, style: Style, spans: &mut Vec<Span<'static>>| {
            if text.is_empty() {
                return;
            }
            if !spans.is_empty() {
                spans.push(Span::styled("  ", dim));
            }
            spans.push(Span::styled(text.to_string(), style));
        };
        push(
            &fitted[0],
            Style::default().add_modifier(Modifier::BOLD),
            &mut spans,
        );
        push(
            &fitted[1],
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            &mut spans,
        );
        // A plugin that can write the game — or press its buttons — is marked
        // on its *name*, so the warning survives any shortening. See
        // crate::plugin: `!`/`!!` for memory, `>`/`>>` for input.
        let wrote = plugins
            .iter()
            .any(|p| p.name.contains('!') || p.name.contains('>'));
        push(
            &fitted[2],
            Style::default().fg(if wrote { Color::Yellow } else { Color::Cyan }),
            &mut spans,
        );
        push(&fitted[3], dim, &mut spans);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The live trainer panel.
    ///
    /// Three rectangles, none of which overlap the game picture or each other
    /// (see [`crate::screen::trainer_view::layout`]): the trainer header, the
    /// focused Pokémon's sprite — a **second terminal image**, with its own id —
    /// and the party list or detail view.
    #[cfg(feature = "plugin-trainer")]
    fn render_trainer(&mut self, f: &mut Frame<'_>, area: Rect) {
        if area.width < tv::PANEL_MIN_W || area.height < 4 {
            return;
        }
        // Taken before the plugin borrow below, which holds `&self`.
        let link_row = self.link_summary().map(|(_, long)| long);
        let link_busy = self.link_active();
        self.trainer_rect = Some(area);
        // The focus ring gains and loses the opponent as battles start and
        // end, so a selection index that meant "party slot 2" a frame ago can
        // mean something else now. Snap to the front of the ring on the
        // transition — which in a battle is the opponent, the thing you opened
        // the panel to look at.
        let fighting = self
            .session
            .as_ref()
            .and_then(|s| s.gb.plugin::<TrainerPlugin>())
            .map(|p| {
                let st = p.state();
                st.opponent.is_some() && st.battle != BattleKind::None
            })
            .unwrap_or(false);
        if fighting != self.trainer_fighting {
            self.trainer_fighting = fighting;
            self.trainer_sel = 0;
        }
        // Split borrows: pull everything needed out of `self` before `session`.
        let detail = self.trainer_detail;
        let selected = self.trainer_sel;
        let palette = self.palette;
        let (font_w, font_h) = self.picker.font_size();
        let sig = self.ui_signature();
        let modal = self.overlay != Overlay::None;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(plugin) = session.gb.plugin_mut::<TrainerPlugin>() else {
            return;
        };

        let Some(game) = plugin.game() else {
            let why = plugin
                .refusal()
                .unwrap_or_else(|| "no trainer data for this cartridge".to_string());
            f.render_widget(
                Paragraph::new(why)
                    .wrap(ratatui::widgets::Wrap { trim: true })
                    .style(Style::default().fg(Color::DarkGray))
                    .block(tv::body_block(" Trainer ")),
                area,
            );
            return;
        };

        if !plugin.state().in_game {
            f.render_widget(
                Paragraph::new(
                    "Waiting for a save to load.\n\nThe party area holds \
                     uninitialised bytes until the game reaches the overworld, \
                     so nothing is shown rather than nonsense.",
                )
                .wrap(ratatui::widgets::Wrap { trim: true })
                .style(Style::default().fg(Color::DarkGray))
                .block(tv::body_block(" Trainer ")),
                area,
            );
            return;
        }

        // **One focus drives the picture and the numbers.** The sprite used to
        // switch to the opponent on its own while the stat block kept showing
        // the player's party, so during a battle the panel showed one
        // Pokémon's face over another's statistics. `Tab` walks the ring, which
        // in a battle starts at the opponent.
        let focus = tv::focus_at(plugin.state(), selected);
        let species =
            focus.and_then(|fo| tv::focused_mon(plugin.state(), fo).map(|m| m.species));

        let want_sprite = species.is_some() && session.sprite_image.is_some();
        // The team strip is progressive: asked for only in a trainer battle
        // with a roster that read back, and granted only if the panel has the
        // width and the spare rows. Where it is refused the roster still shows
        // — as the marker line inside `[ OPPONENT ]`.
        let team: Vec<terminalgb_plugin_trainer::TeamMon> =
            if plugin.state().battle == BattleKind::Trainer {
                plugin.state().enemy_team.clone()
            } else {
                Vec::new()
            };
        // Not in the detail view: that is a request for a page of numbers, and
        // three rows of pictures there costs the bottom of the stat block. The
        // roster is still reported, as a caption under the numbers.
        let want_team = !team.is_empty() && !detail && session.team_image.is_some();
        // Compose the text FIRST and tell the layout how many rows it wants:
        // the width does not depend on the pictures, and the pictures must only
        // be granted space the numbers do not need. A battle's `[ OPPONENT ]`
        // block is a dozen rows on its own, and it must never be the reason the
        // party falls off the bottom.
        let width = area.width.saturating_sub(2) as usize;
        let mut header = tv::header_lines(plugin.state(), game.label(), width);
        // The cable, where the player is already looking. Same source as the
        // Status row's `Link`, so the two cannot drift.
        if let Some(l) = link_row.as_deref() {
            header.push(tv::link_line(l, link_busy, width));
        }
        let lines = tv::body_lines(plugin.state(), focus, detail, width);
        // Laid out against the whole column, then **centred as a block** so the
        // panel sits at the same height as the picture beside it — which
        // `fit_metrics` has always centred. See `PanelLayout::centre_in`.
        let l = tv::layout_with_team(area, want_sprite, want_team, lines.len() as u16)
            .centre_in(area, lines.len() as u16);
        // The panel's own rectangle is the frame it actually drew, not the
        // column it was offered: the "never touches the picture" invariant is
        // about drawn cells, and after centring those are a subset.
        self.trainer_rect = Some(l.frame);
        let title = if detail {
            " Trainer · d back · Tab next "
        } else {
            " Trainer · d detail · Tab next "
        };

        // Encode both pictures before dropping the plugin borrow.
        let mut sprite_ready = false;
        if l.sprite.height > 0 {
            if let Some(species) = species {
                if session.sprite_key != Some((species, palette)) {
                    if let Some(img) = plugin.sprite(species) {
                        encode_sprite(
                            &img.pixels,
                            img.width,
                            img.height,
                            palette,
                            &mut session.sprite_rgba,
                        );
                        session.sprite_dims = (img.width as u32, img.height as u32);
                        session.sprite_key = Some((species, palette));
                    } else {
                        session.sprite_key = None;
                        session.sprite_dims = (0, 0);
                        session.sprite_rgba.clear();
                    }
                }
                sprite_ready = session.sprite_dims.0 > 0;
            }
        }
        if l.team.height > 0 {
            // Keyed on species *and* whether each is standing, so the strip is
            // recomposed exactly when one of them faints and not once a frame.
            let key: Vec<(u8, bool)> =
                team.iter().map(|m| (m.species, m.is_fainted())).collect();
            if session.team_key.as_ref().map(|(k, p)| (k, *p)) != Some((&key, palette)) {
                session.team_dims =
                    compose_team_strip(plugin, &team, palette, &mut session.team_rgba)
                        .unwrap_or((0, 0));
                session.team_key = Some((key, palette));
            }
        }

        f.render_widget(tv::panel_block(title), l.frame);
        f.render_widget(Paragraph::new(header), l.header);
        f.render_widget(Paragraph::new(lines), l.body);

        // The panel is drawn *before* the game picture, so `ui_sig` still holds
        // the previous frame's signature here — comparing against it is exactly
        // the "something may have painted over the image" test.
        let ui_changed = self.ui_sig != Some(sig);
        // A picture that stops being drawn does not stop being displayed: a
        // Kitty placement lives until it is deleted. Opening the detail view
        // retires the team strip and a battle ending retires the sprite, and
        // without this the retired picture hangs over whatever replaced it.
        let show_sprite = sprite_ready && l.sprite.height > 0;
        let show_team = l.team.height > 0 && session.team_dims.0 > 0;
        let mut retired = false;
        if session.sprite_shown && !show_sprite {
            if let Some(img) = session.sprite_image.as_mut() {
                let seq = img.erase().to_string();
                session.pending_wire.push_str(&seq);
                retired = true;
            }
        }
        if session.team_shown && !show_team {
            if let Some(img) = session.team_image.as_mut() {
                let seq = img.erase().to_string();
                session.pending_wire.push_str(&seq);
                retired = true;
            }
        }
        session.sprite_shown = show_sprite;
        session.team_shown = show_team;
        self.full_redraw |= retired;
        if show_sprite {
            let (sw, sh) = session.sprite_dims;
            let (rect, _) = fit_metrics(l.sprite, sw, sh, font_w, font_h);
            self.sprite_rect = place_panel_image(
                f,
                session.sprite_image.as_mut(),
                &session.sprite_rgba,
                (sw, sh),
                rect,
                ui_changed,
                modal,
                &mut session.pending_wire,
            );
        }
        if show_team {
            let (tw, th) = session.team_dims;
            let (rect, _) = fit_metrics(l.team, tw, th, font_w, font_h);
            place_panel_image(
                f,
                session.team_image.as_mut(),
                &session.team_rgba,
                (tw, th),
                rect,
                ui_changed,
                modal,
                &mut session.pending_wire,
            );
        }
    }

    #[cfg(not(feature = "plugin-trainer"))]
    fn render_trainer(&mut self, _f: &mut Frame<'_>, _area: Rect) {}

    /// The live stats panel (`t`), showing what the telemetry stream carries:
    /// displayed and emulated fps, emulated speed as a percentage of real
    /// hardware, the per-stage millisecond breakdown, bytes on the wire, the
    /// dropped/identical frame counters, and the audio queue when a host stream
    /// is attached.
    ///
    /// It is given **its own rectangle**, never drawn over the picture. That is
    /// not cosmetic: a terminal graphics image owns whole cells, so text drawn
    /// into them replaces that part of the image — and with Kitty's Unicode
    /// placeholders it removes the placeholders that make the picture update at
    /// all, which froze the game on screen while the overlay was up.
    fn render_stats(&mut self, f: &mut Frame<'_>, area: Rect) {
        self.stats_rect = Some(area);
        let Some(s) = self.session.as_ref() else {
            return;
        };
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Emulated speed, not display speed: the game runs at 100 % even when
        // the terminal cannot repaint that fast.
        let speed_pct = s.emu_fps as f64 / DMG_HZ as f64 * 100.0;
        let mut lines = vec![
            Line::from(format!("fps  {:.0} / {:.0} emu", s.fps, s.emu_fps)),
            // What the *terminal* can take, measured. `fps` above is what we
            // issued; without this line the two cannot be told apart, which is
            // exactly how a counter that could only read 59 went unnoticed.
            Line::from(format!("term {}", self.termcost_label())),
            Line::from(format!("spd  {speed_pct:.0}% of real")),
            Line::from(format!("emu  {:.2}ms", s.t_emulate)),
            Line::from(format!("pal  {:.2}ms", s.t_palette)),
            Line::from(format!("up   {:.2}ms", s.t_upscale)),
            Line::from(format!("enc  {:.2}ms", s.t_encode)),
            Line::from(format!("tx   {}/f", human_bytes_f(s.tx_per_frame))),
            Line::from(format!(
                "via  {}",
                s.transport_label(self.picker.protocol_type())
            )),
            Line::from(format!("drop {}", s.frames_dropped)),
            Line::from(format!("same {}", s.frames_identical)),
            Line::from(format!("pc   {:04X}", s.gb.debug_pc())),
            Line::from(format!("bank {}", s.gb.debug_rom_bank())),
        ];
        lines.push(Line::from(format!("vol  {}", output_volume::label())));
        if let Some(a) = s.audio_sample() {
            lines.push(Line::from(format!("aq   {}", a.queue_depth)));
            lines.push(Line::from(format!("aur  {}", a.underruns)));
        }
        lines.truncate(area.height.saturating_sub(2) as usize);
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" stats "))
                .style(Style::default().fg(Color::Green)),
            area,
        );
    }

    /// Transient save/load/speed notice. Drawn into a row of its own (the
    /// footer, or a reserved row in fullscreen) — never over the picture, for
    /// the reason given on [`App::render_stats`].
    /// Music mode's own help. Short on purpose: the footer already lists the
    /// keys, so what this adds is the *promise* — what "title theme" means, and
    /// why it is not a soundtrack.
    #[cfg(feature = "music")]
    fn render_music_help(&self, f: &mut Frame<'_>) {
        let area = popup_area(f.area(), 66, 66);
        f.render_widget(Clear, area);
        let d = Style::default().fg(Color::DarkGray);
        let dir = music_dir(&self.config);
        let text = vec![
            Line::from(Span::styled(
                "Music mode",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  ←/→ ↑/↓   browse — the music keeps playing"),
            Line::from("  ⏎         play the selected album"),
            Line::from("  w         save what you are hearing as a WAV"),
            Line::from("  c         next cover candidate"),
            Line::from("  space     pause          g   play the game properly"),
            Line::from("  , / . m   volume / mute   r   rescan"),
            Line::from("  q / esc   back to the ROM picker"),
            Line::from(""),
            Line::from(Span::styled("What you are hearing", d)),
            Line::from(Span::styled(
                "  the TITLE THEME — what the cartridge plays when it boots.",
                d,
            )),
            Line::from(Span::styled(
                "  A Game Boy has no \"play track 5\" interface, so a whole",
                d,
            )),
            Line::from(Span::styled(
                "  soundtrack needs per-game knowledge (a sound test, or the",
                d,
            )),
            Line::from(Span::styled(
                "  game's sound driver). This does not claim it.",
                d,
            )),
            Line::from(""),
            Line::from(Span::styled("The cover", d)),
            Line::from(Span::styled(
                "  a frame from the game's own title screen, found in one",
                d,
            )),
            Line::from(Span::styled(
                "  headless boot. Titles animate in, so several candidates are",
                d,
            )),
            Line::from(Span::styled(
                "  kept — `c` cycles them, and each starts the music at its own",
                d,
            )),
            Line::from(Span::styled("  moment.", d)),
            Line::from(""),
            Line::from(Span::styled(
                "  Nothing is written beside your ROM: no save file is opened.",
                Style::default().fg(Color::Indexed(120)),
            )),
            Line::from(Span::styled(format!("  WAVs go to {}", dir.display()), d)),
            Line::from(Span::styled("  docs/music-mode.md", d)),
        ];
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" ? ")),
            area,
        );
    }

    fn render_toast(&self, f: &mut Frame<'_>, area: Rect) {
        let Some(msg) = self.toast_text() else { return };
        let text = format!(" {msg} ");
        let w = (text.chars().count() as u16).min(area.width);
        if w == 0 || area.height == 0 {
            return;
        }
        let rect = Rect::new(area.x, area.y, w, 1);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(text).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            rect,
        );
    }

    /// Draw the game picture, centred and upscaled to fill `area`.
    ///
    /// The display is deliberately *borderless* (the framed regions are the
    /// status/controls panels) so the picture gets every row of height — height
    /// is the binding constraint for a 10:9 image in a terminal whose cells are
    /// ~twice as tall as wide.
    ///
    /// Scaling uses `Resize::Scale`, **not** `Resize::Fit`: Fit refuses to
    /// enlarge an image smaller than the area (it caps the target at the source
    /// size), which is exactly why the old frontend was "limited to small" and
    /// faked enlargement with a capped integer pre-scale. `Scale` upscales the
    /// source to fill the centred rect at pixel resolution with nearest-
    /// neighbour (crisp pixels, correct aspect), so size steps are as fine as
    /// the protocol allows and re-fit is automatic on resize.
    /// Build the [`TermImage`] encoder for the active protocol, or `None` when
    /// the fast path does not apply.
    ///
    /// Sixel and halfblocks have no way to say "draw this image over these
    /// cells" — sixel carries its own pixel grid and halfblocks *is* text — so
    /// they keep the resize-to-the-area path. Kitty and iTerm2 both scale a
    /// cell-addressed placement for us, which is what makes the transmission
    /// size independent of the window.
    fn new_term_image(&self) -> Option<TermImage> {
        if self.config.render_path == RenderPath::Legacy {
            return None;
        }
        let wire = match self.picker.protocol_type() {
            ProtocolType::Kitty => Wire::Kitty,
            ProtocolType::Iterm2 => Wire::Iterm2,
            _ => return None,
        };
        let mut img = TermImage::new(wire, 0x0067_6201);
        // Kitty can take the frame out of shared memory instead of off the
        // wire — about sixty bytes per frame rather than a couple of kilobytes.
        // The answer was worked out once at startup (see `probe_shm_support`);
        // it is a property of the terminal, not of the session.
        if wire == Wire::Kitty && shm_supported() {
            img.set_kitty_shm(true);
        }
        Some(img)
    }

    /// The trainer view's sprite slot. A **distinct** image id from the game
    /// picture: Kitty stores images by id, so sharing one would have the sprite
    /// and the game overwrite each other every frame.
    fn new_sprite_image(&self) -> Option<TermImage> {
        self.new_term_image()
            .map(|t| TermImage::new(t.wire(), 0x0067_6202))
    }

    /// The opposing trainer's team strip — a **third** terminal image, so a
    /// third id. All three rules from `docs/terminal-render.md` apply to it:
    /// its own id (never a fresh one per frame), its own cell rectangle, and
    /// the cells it covers marked `skip`.
    fn new_team_image(&self) -> Option<TermImage> {
        self.new_term_image()
            .map(|t| TermImage::new(t.wire(), 0x0067_6203))
    }

    /// The upscale factor actually applied before transmission (see
    /// [`crate::screen::config::TransmitScale`]) — usually smaller than the
    /// factor shown in the status
    /// panel, because the terminal does the rest of the enlargement for free.
    /// The terminal's measured per-frame cost and the ceiling it implies, or an
    /// honest "we cannot tell".
    ///
    /// **This is a ceiling, not a present count**, and it is labelled so. No
    /// terminal protocol reports "I painted your image" — the Kitty graphics
    /// response acknowledges the command, and a fence cannot report a frame the
    /// terminal coalesced and never painted. What it *can* report is how long
    /// the terminal takes to accept one, which bounds what it can present.
    fn termcost_label(&self) -> String {
        match (self.termcost.per_frame(), self.termcost.ceiling_fps()) {
            (Some(d), Some(fps)) => {
                let ms = d.as_secs_f32() * 1000.0;
                if fps >= DMG_HZ {
                    // Not the bottleneck: say so rather than printing a
                    // four-digit fps that invites the wrong conclusion.
                    format!("{ms:.1}ms ok")
                } else {
                    format!("{ms:.1}ms ≤{fps:.0}fps")
                }
            }
            _ if !self.termcost.enabled() => "n/a no CPR".to_string(),
            _ => "measuring".to_string(),
        }
    }

    fn transmit_factor(&self) -> u8 {
        self.config
            .transmit_scale
            .factor_for(self.upscaler.algorithm(), self.upscale_factor)
    }

    /// The upscaler label for the status panel and toasts — built from the
    /// factor the *user* chose, not the (possibly smaller) transmission factor.
    fn upscale_label(&self) -> String {
        format!(
            "{} x{}",
            self.upscaler.algorithm().name(),
            self.upscale_factor
        )
    }

    /// The Status panel's `PPU` row — which rendering engine drew the picture.
    /// With no ROM loaded there is no PPU to ask, so it reports what the next
    /// one will get rather than inventing a value.
    fn ppu_label(&self) -> &'static str {
        match self.session.as_ref() {
            Some(s) => s.gb.render_mode().name(),
            None => self
                .config
                .ppu
                .unwrap_or_else(crate::gpu::RenderMode::from_env)
                .name(),
        }
    }

    fn render_display(&mut self, f: &mut Frame<'_>, area: Rect) {
        let (font_w, font_h) = self.picker.font_size();
        // Kitty addresses placeholder rows/columns with a fixed table of
        // combining marks, so a placement cannot exceed `MAX_CELLS` cells on
        // either axis. Clamp the *area* rather than the fitted rectangle, so
        // the picture stays aspect-correct and centred on a very large display.
        let area = if self
            .session
            .as_ref()
            .and_then(|s| s.term_image.as_ref())
            .map(TermImage::wire)
            == Some(Wire::Kitty)
        {
            let max = crate::termimage::MAX_CELLS;
            let w = area.width.min(max);
            let h = area.height.min(max);
            Rect::new(
                area.x + (area.width - w) / 2,
                area.y + (area.height - h) / 2,
                w,
                h,
            )
        } else {
            area
        };
        let (rect, scale) = fit_metrics(area, 160, 144, font_w, font_h);
        // Temporary instrumentation for the scaling bug: dump the real numbers
        // (terminal cells, font_size, area, fitted rect, scale, source dims)
        // once per distinct layout. Read via `GB_DEBUG_LAYOUT=<file>`.
        if self.dbg_last != Some((area, rect, scale)) {
            let term = f.area();
            debug_layout(&format!(
                "[render] term_cells={}x{} font_size=({font_w},{font_h}) \
                 area={}x{}@{},{} rect={}x{}@{},{} scale={scale:.3} \
                 src=160x144 disp_px={}x{}",
                term.width,
                term.height,
                area.width,
                area.height,
                area.x,
                area.y,
                rect.width,
                rect.height,
                rect.x,
                rect.y,
                rect.width as u32 * font_w as u32,
                rect.height as u32 * font_h as u32,
            ));
            self.dbg_last = Some((area, rect, scale));
        }

        // Anything that paints over the picture (a modal popup, the stats
        // panel appearing and reflowing the layout, the toast row) may have
        // destroyed part of the terminal's copy of the image, so the next
        // transmission must be unconditional.
        let sig = self.ui_signature();
        let ui_changed = self.ui_sig != Some(sig);
        self.ui_sig = Some(sig);
        // A modal popup is drawn *over* the picture, and the picture is written
        // after the frame is flushed — so while one is up we deliberately do
        // not retransmit, leaving the last frame on screen behind the popup.
        // Emulation is paused under a modal overlay anyway, so it is not stale.
        let modal = self.overlay != Overlay::None;

        // Split borrows: everything we need out of `self` before `session`.
        let palette = self.palette;
        let picker = self.picker;
        let tx_factor = self.transmit_factor();
        let upscaler = &mut self.upscaler;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        s.scale = scale;
        s.disp_cells = (rect.width, rect.height);
        s.disp_origin = (rect.x, rect.y);
        s.disp_px = (
            rect.width as u32 * font_w as u32,
            rect.height as u32 * font_h as u32,
        );
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        // 1. Palette-map into a reused buffer (was a fresh Vec per frame).
        let t = Instant::now();
        let mapped = s.prep.map(s.gb.image(), palette);
        s.t_palette = telemetry::ms(t.elapsed());

        // 2. Upscale, at the *transmission* factor — never at the display
        //    resolution. For a real filter this is the visual effect and must
        //    happen here; for `nearest` it is the terminal's job (see
        //    `TransmitScale`).
        let t = Instant::now();
        if upscaler.factor() != tx_factor {
            upscaler.set_factor(tx_factor);
        }
        let (tw, th) = upscaler.output_size(160, 144);
        let scaled = upscaler.apply(mapped, 160, 144);
        s.t_upscale = telemetry::ms(t.elapsed());
        s.tx_px = (tw as u32, th as u32);

        // 3. Encode + place.
        match s.term_image.as_mut() {
            Some(term) => {
                if ui_changed {
                    term.invalidate();
                }
                let t = Instant::now();
                let placement = term.place(
                    scaled,
                    tw as u32,
                    th as u32,
                    CellRect::new(rect.x, rect.y, rect.width, rect.height),
                );
                s.t_encode = telemetry::ms(t.elapsed());
                if !placement.changed {
                    s.frames_identical += 1;
                }
                s.tx_bytes = placement.payload_bytes;
                s.tx_window += placement.payload_bytes;
                // The sequence is written to the terminal *after* the frame is
                // flushed (see the main loop), not stuffed into a cell symbol:
                // a 92 KB symbol makes `ratatui`'s diff re-emit the whole
                // screen every frame, and anything drawn over that cell stops
                // the picture updating. See `crate::termimage`.
                s.pending_wire.clear();
                if placement.changed && !modal {
                    s.pending_wire.push_str(placement.seq);
                }
                // Every cell the picture covers is marked `skip`, so `ratatui`
                // leaves the image alone.
                let buf = f.buffer_mut();
                for y in 0..rect.height {
                    for x in 0..rect.width {
                        if let Some(cell) = buf.cell_mut((rect.x + x, rect.y + y)) {
                            cell.set_skip(true);
                        }
                    }
                }
            }
            None => {
                // Legacy path: hand `ratatui-image` the frame and let it resize
                // to the display rectangle. Rebuilt only when the picture
                // changes — it used to be rebuilt every single frame, which
                // re-hashed and re-encoded a still screen sixty times a second.
                let t = Instant::now();
                let hash = crate::termimage::fnv1a(scaled);
                if hash != s.legacy_hash || s.image.is_none() || ui_changed {
                    let buffer = image::ImageBuffer::from_raw(
                        tw as u32,
                        th as u32,
                        scaled.to_vec(),
                    )
                    .expect("upscaled framebuffer size matches output dimensions");
                    s.image = Some(
                        picker.new_resize_protocol(DynamicImage::ImageRgba8(buffer)),
                    );
                    s.legacy_hash = hash;
                } else {
                    s.frames_identical += 1;
                }
                s.t_encode = telemetry::ms(t.elapsed());
                if let Some(proto) = s.image.as_mut() {
                    let image = StatefulImage::default().resize(Resize::Scale(None));
                    f.render_stateful_widget(image, rect, proto);
                }
                // This path writes through `ratatui`'s buffer, so its bytes are
                // only visible at the backend's writer — and they are billed to
                // the *previous* draw, because this one has not been flushed
                // yet. Reporting the delta is what makes `Sent` true here
                // instead of the hard-coded `0 B` it used to show.
                let wrote = wrote_total();
                s.tx_bytes = wrote.saturating_sub(s.wrote_mark) as usize;
                s.wrote_mark = wrote;
                s.tx_window += s.tx_bytes;
            }
        }

        // Displayed-frame accounting.
        s.disp_count += 1;
        s.disp_frames += 1;
        s.frames_dropped += s.skipped_since_display.saturating_sub(1);
        s.skipped_since_display = 0;
    }

    fn render_sidebar(&mut self, f: &mut Frame<'_>, area: Rect) {
        // A plugin can answer the link cable and (if it declared the
        // capability) write memory, so the user must be able to *see* that one
        // is attached without going looking. `!` marks a plugin that may write
        // emulated memory, `!!` one that has — see crate::plugin.
        // **One row per plugin**, not one joined line. Two bugs lived here, both
        // found by screenshotting the real frontend rather than by any unit
        // test on the row's content, and both defeating the thing
        // docs/plugins.md §4 promises — "a plugin cannot be active and
        // invisible":
        //
        //   1. The box was a fixed `Length(13)`, which fits exactly eleven
        //      content rows, and the plugin row is pushed *last* — so it was
        //      clipped away entirely.
        //   2. `plugin_status_line()` joins every plugin with " | " and the row
        //      truncates to 18 columns, so with the always-attached trainer
        //      view first, a *second* plugin — the link partner, the one that
        //      can actually change your game — vanished behind the ellipsis.
        let plugins: Vec<crate::plugin::PluginStatus> = self
            .session
            .as_ref()
            .map(|s| s.gb.plugin_statuses())
            .unwrap_or_default();
        // A crashed machine has stopped writing the save file. That is the most
        // important thing on this panel while it is true, so it gets its own row
        // (and the box grows to fit it — see the clipping bug above).
        let fault = self.fault_text().map(str::to_string);

        // Status. `Sent` is the size actually transmitted to the terminal each
        // frame, which the terminal then stretches to `Scale`; keeping those
        // two apart is what holds the frame rate up on a large window.
        let (title, mbc, scale, fps, emu_fps, paused, sent, tx, via) = self
            .session
            .as_ref()
            .map(|s| {
                (
                    s.entry.title.clone(),
                    s.entry.mbc,
                    s.scale,
                    s.fps,
                    s.emu_fps,
                    s.paused,
                    s.tx_px,
                    s.tx_per_frame,
                    s.transport_label(self.picker.protocol_type()),
                )
            })
            .unwrap_or((
                String::new(),
                "",
                1.0,
                0.0,
                0.0,
                false,
                (160, 144),
                0.0,
                "-",
            ));
        let mut status = vec![
            kv("ROM", &truncate(&title, 18)),
            kv("MBC", mbc),
            kv("Scale", &format!("{scale:.1}x")),
            kv(
                "Sent",
                &format!("{}x{} {}", sent.0, sent.1, human_bytes_f(tx)),
            ),
            kv(
                "Proto",
                &format!("{} {via}", protocol_name(self.picker.protocol_type())),
            ),
            // Which input mode is live. Without this the fallback's different
            // feel — a held direction that pauses before it walks — reads as
            // the emulator being broken rather than the terminal being basic.
            kv("Input", self.input.label()),
            kv("Palette", self.palette.name()),
            kv("Upscale", &self.upscale_label()),
            // Which of the two emulated rendering engines is live. Not a
            // picture-quality setting — palettes and upscaling above are
            // unaffected by it — but the two *can* disagree on a game that
            // writes PPU registers mid-scanline, so which one produced the
            // picture must never be a guess. See `crate::gpu::RenderMode`.
            kv("PPU", self.ppu_label()),
            kv("Speed", SPEED_LADDER[self.speed].label),
            kv("Slot", &self.slot.to_string()),
            // "Am I live?" must never be a guess — see `App::toggle_video`.
            kv("Video", &self.video_status()),
            // Same rule for sound. A device that never opened, or one that has
            // started erroring, used to be indistinguishable from a quiet game.
            kv("Audio", &truncate(&self.audio_status(), 18)),
            // Playback volume, its own row beside the device status: "no sound"
            // has two completely different causes (no device, or turned down)
            // and the player has to be able to tell them apart at a glance.
            kv("Volume", &output_volume::label()),
            kv(
                "FPS",
                &if paused {
                    "paused".to_string()
                } else {
                    format!("{fps:.0} · emu {emu_fps:.0}")
                },
            ),
            // The two halves of a displayed frame. `FPS` above is **ours**:
            // pictures we issued. `Term` is the terminal's — how long it takes
            // to accept one, and therefore the most it could possibly present.
            // They used to be indistinguishable, which is how "reports 59, is
            // clearly rendering slower" went unexplained. See
            // `crate::screen::termcost` for why there is no true present count.
            kv("Term", &self.termcost_label()),
        ];
        // A second console must never be running invisibly — the same rule the
        // plugin row follows. One mechanism with the trainer panel, so the two
        // cannot drift (`link_summary`).
        if let Some((short, _)) = self.link_summary() {
            status.push(kv("Link", &truncate(&short, 18)));
        }
        // The `!` / `!!` marks travel with the name, so "this one may write my
        // game" survives truncation of the detail text.
        for p in &plugins {
            let detail = if p.detail.is_empty() {
                p.name.clone()
            } else {
                format!("{}: {}", p.name, p.detail)
            };
            status.push(kv("Plugin", &truncate(&detail, 18)));
        }
        if fault.is_some() {
            status.push(kv("Fault", "SAVE NOT WRITTEN"));
        }
        // Sized from `status` itself, plus the two border rows. A hand-counted
        // constant here is the bug this box has had twice: it was `13 + plugins`
        // against a twelve-line list, so with two plugins attached the second
        // one was clipped away and `an_attached_plugin_is_visible_in_the_
        // rendered_status_panel` failed — a plugin *was* active and invisible,
        // which is exactly what docs/plugins.md §4 promises cannot happen.
        // Adding a status row must not silently cost a plugin its row again —
        // the `Fault` row above is one such addition and costs nothing, and so
        // is the `Input` row this fallback added.
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(status.len() as u16 + 2),
                Constraint::Min(0),
            ])
            .split(area);
        f.render_widget(
            Paragraph::new(status)
                .block(Block::default().borders(Borders::ALL).title(" Status ")),
            split[0],
        );

        // Controls.
        let k = &self.config.keys;
        let mut controls = vec![
            Line::from("Arrows  move"),
            Line::from(format!("{}       A", k.a)),
            Line::from(format!("{}       B", k.b)),
            Line::from(format!("{}       Start", k.start)),
            Line::from(format!("{}       Select", k.select)),
            Line::from(""),
            Line::from("p pause     R reset"),
            Line::from("+/- speed   t stats"),
            // Volume sits high in this list on purpose: the panel is clipped
            // from the bottom on a short window, and an everyday control must
            // not be the first casualty. (A screenshot key can afford to be.)
            Line::from(", . volume  m mute"),
            Line::from("1-3 slot"),
            Line::from("F5 save     F8 load"),
            Line::from("L load ROM  f full"),
            Line::from("[ palette   ? help"),
            Line::from("] upscale   } factor"),
            Line::from("c shot      C raw shot"),
            Line::from("v stream    V record"),
        ];
        // Only when there is a cable — a key that does nothing is worse than no
        // key at all — and it says which of the two things it will do.
        if let Some(s) = self.session.as_ref() {
            if s.link.is_some() {
                controls.push(Line::from("W other console"));
            } else if s.peer.is_some() {
                controls.push(Line::from("W cable on/off"));
            }
        }
        f.render_widget(
            Paragraph::new(controls)
                .block(Block::default().borders(Borders::ALL).title(" Controls ")),
            split[1],
        );
    }

    /// The Plugins section of the help overlay: what exists, how to arm it, and
    /// what is attached right now.
    ///
    /// Plugins are deliberately opt-in — an unmodified `config.toml` runs an
    /// unmodified machine — but "opt-in" is only a feature if the options are
    /// findable. Before this the link partner was documented in
    /// `docs/plugins.md` and nowhere the player could see, so the only way to
    /// learn it existed was to read the repository.
    fn plugin_help(&self) -> Vec<Line<'static>> {
        let d = Style::default().fg(Color::DarkGray);
        let attached: Vec<crate::plugin::PluginStatus> = self
            .session
            .as_ref()
            .map(|s| s.gb.plugin_statuses())
            .unwrap_or_default();
        let mut out = vec![
            Line::from(""),
            Line::from("Plugins — nothing loads unless you name it"),
            Line::from(Span::styled(
                "  config.toml:  plugins = gen1-link      (or GB_PLUGINS=…)",
                d,
            )),
            Line::from(vec![
                Span::styled("  trainer    ", Style::default().fg(Color::Cyan)),
                Span::raw("this panel · always on · read-only"),
            ]),
            Line::from(vec![
                Span::styled("  gen1-link  ", Style::default().fg(Color::Cyan)),
                Span::raw("answers the link cable and trades a Pokémon in"),
            ]),
            Line::from(Span::styled(
                "             gen1_trade = ~/mewtwo.trade, else a built-in offer",
                d,
            )),
            Line::from(Span::styled(
                "             it CAN write your game — marked ! in Status",
                Style::default().fg(Color::Yellow),
            )),
        ];
        if !attached.is_empty() {
            let names: Vec<String> = attached.iter().map(|p| p.name.clone()).collect();
            out.push(Line::from(Span::styled(
                format!("  attached now: {}", names.join(", ")),
                d,
            )));
        }
        // The boot ROM is opt-in in exactly the same sense, and findable only
        // if it is written down where the player looks. Nothing is bundled and
        // nothing is fetched — see `crate::boot`.
        let booting = self
            .session
            .as_ref()
            .is_some_and(|s| s.gb.boot_rom_mapped());
        out.push(Line::from(""));
        out.push(Line::from("Boot ROM — optional, yours, never bundled"));
        out.push(Line::from(Span::styled(
            "  drop a dump of your own hardware's boot ROM at",
            d,
        )));
        out.push(Line::from(Span::styled(
            "  ~/.config/terminalgb/dmg_boot.bin   (256 bytes)",
            d,
        )));
        out.push(Line::from(Span::styled(
            "  ~/.config/terminalgb/cgb_boot.bin   (2304 bytes)",
            d,
        )));
        out.push(Line::from(Span::styled(
            if booting {
                "  in use for this game — the logo you just saw was real"
            } else {
                "  none found: the boot state is reconstructed instead"
            },
            d,
        )));
        out
    }

    fn render_help(&self, f: &mut Frame<'_>) {
        // On the shelf, show the shelf's help. The full overlay is longer than
        // the popup and does not scroll, so a music section appended to the end
        // of it is a section nobody can read — which is exactly the
        // "discoverable only in the docs" failure the plugin list was added to
        // fix.
        #[cfg(feature = "music")]
        if matches!(self.screen, Screen::Shelf) {
            self.render_music_help(f);
            return;
        }
        let area = popup_area(f.area(), 60, 60);
        f.render_widget(Clear, area);
        let mut text = vec![
            Line::from(Span::styled(
                "Help",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Picker"),
            Line::from("  ↑/↓ j/k   move        enter   play"),
            Line::from("  /         filter      r       rescan"),
            Line::from("  .         resume last g/G     top/bottom"),
            Line::from("  M         music mode  q/esc   quit"),
            Line::from(""),
            Line::from("Game"),
            Line::from("  arrows    move        a/s     A / B"),
            Line::from("  x/z       Start/Sel   p       pause"),
            Line::from("  +/-       speed        t       stats overlay"),
            Line::from(
                "  , / .     volume down / up      m   mute (remembers the level)",
            ),
            Line::from(Span::styled(
                "  volume is playback only — the emulated machine is untouched; \
                 `volume = 50` in config.toml sets the starting level",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from("  1-3       save slot    F5/F8   save / load"),
            Line::from("  R         reset       L       load ROM"),
            Line::from("  f         fullscreen  [       palette"),
            Line::from("  ]         upscaler    }       upscale factor"),
            Line::from("  c         screenshot  C       screenshot (raw 160x144)"),
            Line::from(
                "  v         live feed to `video_out` on/off — starts OFF; a camera \
                 stays open and shows an idle card",
            ),
            Line::from(
                "  V         record a timestamped file instead (see docs/video-out.md)",
            ),
            // The one question this help could not answer. Nothing told anyone
            // the feed is the game and only the game, so "how do I put the
            // panel in it?" had no shortcut and no explanation either.
            Line::from(Span::styled(
                "  the feed carries the GAME PICTURE ONLY — no panels, by design. \
                 To stream the panels too, capture this window in OBS and use \
                 OBS Virtual Camera (docs/stream-panel-routes.md); this window is \
                 titled \"TerminalGB\" so a scene can find it",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from("  ?         this help   q       quit"),
            Line::from(""),
            Line::from("Link cable (two consoles, one wire)"),
            Line::from("  W         swap which console you are playing"),
            Line::from(Span::styled(
                "  set `link = <rom>` in config.toml to boot a second cartridge                  joined to this one by a real link cable. Both run; only the one                  on screen takes your keys. See docs/link-cable.md — including                  what Gold/Silver need before the Time Capsule will open",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from("AI agent (Claude plays Pokémon)"),
            Line::from("  A         commentary panel   G   agent on/off"),
            Line::from(Span::styled(
                "  it presses the buttons; it never writes to the game",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from("Trainer view (Pokémon Red/Blue)"),
            Line::from("  T         show/hide   d       detail for the focused one"),
            Line::from("  tab       next        shift-tab  previous"),
            Line::from(Span::styled(
                "  in a battle tab starts at the opponent, then your party",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  read-only: it never writes to the game",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        // Music mode is a whole screen, so it gets its own section — and the
        // section is where the promise is written down: what it plays is the
        // *title theme*, because booting a cartridge is the only way to get
        // audio out of one without per-game knowledge. Saying so here is the
        // difference between an honest feature and an overclaim.
        #[cfg(feature = "music")]
        text.extend([
            Line::from(""),
            Line::from("Music mode  (M from the picker, or `terminal music`)"),
            Line::from("  ←/→       browse      ⏎       play this album"),
            Line::from("  w         save what you are hearing as a WAV"),
            Line::from("  c         next cover  space   pause"),
            Line::from("  g         play the game properly   q/esc   leave"),
            Line::from(Span::styled(
                "  cover art is a frame from the game's own title screen, picked                  from one headless boot; several candidates are kept because                  titles animate in, which is what `c` cycles",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  what you hear is the TITLE THEME — what the cartridge plays                  when it boots. There is no \"play track 5\" interface on a Game                  Boy, so a full soundtrack needs per-game knowledge (a sound                  test, or the game's sound driver) and this does not claim it",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  nothing is written beside your ROM: no save file is opened,                  and WAVs go to music_dir (docs/music-mode.md)",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        // **Plugins have to be discoverable from inside the emulator.** They
        // are opt-in by design — nothing loads unless it is named — but that
        // only works if the names are somewhere a player will find them, and
        // until now the only place was `docs/plugins.md`.
        text.extend(self.plugin_help());
        text.extend([
            Line::from(""),
            Line::from("Battle control (Pokémon Red/Blue, plugins = gen1-battle)"),
            Line::from("  B         start the selected battle   N   next preset"),
            Line::from(Span::styled(
                "  writes the party into RAM for the fight and puts it back \
                 afterwards; it never writes cartridge RAM, so your .sav is \
                 untouched unless you save in-game (docs/gen1/battle-control.md)",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from("Wild-encounter shiny check"),
            Line::from(Span::styled(
                format!(
                    "  a banner names every catchable wild Pokémon's DVs and \
                     whether it would be shiny if traded to Gen 2 — no key \
                     needed, the trainer panel may stay shut (shiny_alert = {})",
                    self.shiny_alert.name()
                ),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  Gen 1 has no shiny Pokémon at all; the claim is only what \
                 Gold/Silver would do with the same DVs",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                match self.input {
                    InputMode::Enhanced => "input: kitty keyboard protocol — exact \
                         press/release, so hold and diagonals work"
                        .to_string(),
                    InputMode::Basic => {
                        "input: basic terminal — no key-release event, so a \
                         held button is let go shortly after the terminal stops \
                         auto-repeating it. Kitty/Ghostty/foot/WezTerm \
                         (enable_kitty_keyboard) give exact hold."
                            .to_string()
                    }
                },
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("screenshots -> {}", screenshot_dir(&self.config).display()),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "press any key to close",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        // A config key that did not apply is repeated here, so it cannot be
        // missed by someone who started the emulator from a launcher and never
        // saw the startup stderr.
        if !self.config_warnings.is_empty() {
            text.push(Line::from(""));
            for w in &self.config_warnings {
                text.push(Line::from(Span::styled(
                    format!("⚠ {w}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(" ? ")),
            area,
        );
    }
}

fn render_quit(f: &mut Frame<'_>) {
    let area = popup_area(f.area(), 34, 20);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from("Quit TerminalGB?"),
            Line::from(""),
            Line::from(Span::styled(
                "y = yes    any other = no",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(ratatui::layout::Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title(" Quit ")),
        area,
    );
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Turn a decoded Gen 1 picture — one palette index per pixel — into the RGBA
/// the terminal image path takes, **with the page behind the Pokémon made
/// transparent** so the panel shows through instead of a pasted-on white box.
///
/// Which pixels count as the page is
/// [`sprite::background_mask`](terminalgb_plugin_trainer::sprite::background_mask): a
/// flood fill from the border, *not* a colour key — Gen 1 art uses the lightest
/// shade inside the figure too, and keying on it punches holes through the
/// middle of the Pokémon.
///
/// Transparent pixels keep their opaque colour in RGB and only carry `A = 0`.
/// That is what makes the fallback safe: a transport that drops alpha (the
/// `GB_TERM_IMAGE=rgb` escape hatch, or a truecolour PNG) renders exactly the
/// opaque picture it renders today rather than a hole full of undefined colour.
#[cfg(feature = "plugin-trainer")]
fn encode_sprite(pixels: &[u8], w: usize, h: usize, palette: Palette, out: &mut Vec<u8>) {
    let bg = terminalgb_plugin_trainer::sprite::background_mask(pixels, w, h);
    out.clear();
    out.reserve(pixels.len() * 4);
    for (i, &px) in pixels.iter().enumerate() {
        let [r, g, b] = palette.shade(px);
        let a = if bg.get(i).copied().unwrap_or(false) {
            0
        } else {
            255
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
}

/// Fit a bar of segments into `width`, shrinking each through **its own ladder
/// of shorter forms** before any of it is dropped.
///
/// Each segment is given longest-first; the bar repeatedly shortens whichever
/// segment is currently costing the most until the whole line fits, and only
/// once nothing can shrink further does it drop segments from the right. That
/// is what stops a narrow window losing the controls entirely — they shorten to
/// `? help · q quit` rather than vanishing — and it is why the ladders are
/// written as *complete alternatives* rather than as a string to truncate: a
/// clipped control hint is worse than a short one, because the reader cannot
/// tell it was clipped.
fn fit_bar(ladders: &[Vec<String>], sep: &str, width: usize) -> Vec<String> {
    let mut at: Vec<usize> = vec![0; ladders.len()];
    let mut live: Vec<bool> = vec![true; ladders.len()];
    let total = |at: &[usize], live: &[bool]| -> usize {
        let parts: Vec<usize> = (0..ladders.len())
            .filter(|&i| live[i] && !ladders[i][at[i]].is_empty())
            .map(|i| ladders[i][at[i]].chars().count())
            .collect();
        parts.iter().sum::<usize>() + sep.chars().count() * parts.len().saturating_sub(1)
    };
    // Shrink the longest segment that still has a shorter form.
    while total(&at, &live) > width {
        let worst = (0..ladders.len())
            .filter(|&i| live[i] && at[i] + 1 < ladders[i].len())
            .max_by_key(|&i| ladders[i][at[i]].chars().count());
        match worst {
            Some(i) => at[i] += 1,
            None => break,
        }
    }
    // Then, only then, drop whole segments from the right.
    while total(&at, &live) > width {
        match (0..ladders.len()).rev().find(|&i| live[i]) {
            Some(i) if i > 0 => live[i] = false,
            _ => break,
        }
    }
    (0..ladders.len())
        .map(|i| {
            if live[i] {
                ladders[i][at[i]].clone()
            } else {
                String::new()
            }
        })
        .collect()
}

/// Lay the picture and the trainer panel out as **one block**: adjacent, the
/// same height, centred together in `body`.
///
/// # Why the leftover space must not land between them
///
/// `fit_metrics` centres the picture inside whatever column it is given, and
/// the panel used to be pinned to the window's right edge. In any window wider
/// than the picture needs — which is every window where the picture is
/// height-limited — that puts *all* the horizontal slack in a band **between**
/// the two, so the picture ends well short of the panel and the two halves read
/// as unrelated things pasted side by side. Centring the block instead moves
/// that slack to the outside edges, where it is symmetric and reads as margin.
///
/// The same argument applies vertically, and is why both pieces get the *same*
/// rectangle height rather than one being centred and the other anchored: a
/// composition has one alignment, not two. `block_h` is the taller of what the
/// picture needs and what the panel needs, so neither is cropped to suit the
/// other, and `PanelLayout::centre_in` then centres the panel's content inside
/// its half exactly as `fit_metrics` centres the picture inside its own.
fn compose_block(
    body: Rect,
    panel_w: u16,
    panel_rows: u16,
    font: (u16, u16),
) -> (Rect, Rect) {
    let column = Rect::new(
        body.x,
        body.y,
        body.width.saturating_sub(panel_w),
        body.height,
    );
    let (pic, _) = fit_metrics(column, 160, 144, font.0, font.1);
    let block_w = (pic.width + panel_w).min(body.width);
    let block_h = pic.height.max(panel_rows).min(body.height);
    let x = body.x + (body.width - block_w) / 2;
    let y = body.y + (body.height - block_h) / 2;
    let game_w = block_w.saturating_sub(panel_w);
    (
        Rect::new(x, y, game_w, block_h),
        Rect::new(x + game_w, y, panel_w, block_h),
    )
}

/// How many columns the trainer panel gets, given the whole width.
///
/// **The panel never takes more than half the window.** It used to take
/// `PANEL_W` (40) whenever the leftovers cleared `MIN_GAME_W`, which is fine at
/// 120 columns and ruinous at 62: the panel took 40, the picture was left 22,
/// and since a Game Boy frame is 2.7× wider than tall in cells, 22 columns is a
/// picture **eight rows high** in a sixty-row pane. The panel does not get
/// better above about 34 columns — its widest tier is designed for 38 — whereas
/// the picture is width-limited in every tall window and can always use more.
/// So the split is capped at half, which roughly doubles the picture in a
/// narrow split and changes nothing at 80 columns or above.
///
/// `PANEL_MIN_W` is a floor rather than a proportion, because a panel narrower
/// than that carries less than it costs; below it `render_trainer` declines and
/// the width goes back to the picture.
#[cfg(feature = "plugin-trainer")]
fn trainer_panel_width(total: u16) -> u16 {
    // Two fifths, so the two halves keep their proportions as the window grows
    // instead of the panel staying a fixed column count beside a picture that
    // does not — but never below the width the sections need to say anything.
    let want = (total * 2 / 5).max(tv::PANEL_MIN_W);
    let fits = want.min(total.saturating_sub(MIN_GAME_W));
    if fits >= tv::PANEL_MIN_W {
        fits
    } else {
        0
    }
}

#[cfg(not(feature = "plugin-trainer"))]
/// Without the trainer plugin the panel does not exist, so it takes no width.
fn trainer_panel_width(_total: u16) -> u16 {
    0
}

/// Place one of the trainer panel's images over a cell rectangle and mark the
/// cells it covers `skip`, returning the rectangle it actually took.
///
/// Shared by the focused Pokémon's sprite and the opposing trainer's team
/// strip because the three rules from `docs/terminal-render.md` apply
/// identically to both, and having them written twice is how one of them ends
/// up with only two: reuse the image's own id (never a fresh one per frame),
/// re-transmit when the UI may have painted over it, and never let text share a
/// cell with a picture.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(feature = "plugin-trainer"), allow(dead_code))]
fn place_panel_image(
    f: &mut Frame<'_>,
    image: Option<&mut TermImage>,
    rgba: &[u8],
    dims: (u32, u32),
    rect: Rect,
    ui_changed: bool,
    modal: bool,
    wire: &mut String,
) -> Option<Rect> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let img = image?;
    if ui_changed {
        img.invalidate();
    }
    let placement = img.place(
        rgba,
        dims.0,
        dims.1,
        CellRect::new(rect.x, rect.y, rect.width, rect.height),
    );
    // While a modal popup is up nothing is retransmitted, for the same reason
    // the game picture is not: the popup is drawn over it.
    if placement.changed && !modal {
        wire.push_str(placement.seq);
    }
    // **Skip only the cells of a picture that is NOT being retransmitted.**
    //
    // `skip` tells ratatui's diff to leave a cell alone, which is right for a
    // static image: text drawn into it would replace part of the picture, and
    // on Kitty it removes the placeholders that make the picture update at all.
    // But it also means the cell is never *erased*, so when the layout moves an
    // image the text that used to be underneath is still on the terminal — and
    // with a transparent sprite background it shows straight through the
    // picture as half-eaten words. (An opaque picture hid this for as long as
    // the sprites had a white box behind them.)
    //
    // On a frame where the picture is transmitted anyway there is nothing to
    // protect: `pending_wire` is flushed *after* `terminal.draw`, so the text
    // ratatui paints into these cells is immediately covered by the image in
    // the same write burst. Letting the diff run on those frames is what erases
    // the ghost.
    if !placement.changed {
        let buf = f.buffer_mut();
        for y in 0..rect.height {
            for x in 0..rect.width {
                if let Some(cell) = buf.cell_mut((rect.x + x, rect.y + y)) {
                    cell.set_skip(true);
                }
            }
        }
    }
    Some(rect)
}

// ---------------------------------------------------------------------------
// Music mode
// ---------------------------------------------------------------------------

/// Where exported WAVs land: the `music_dir` config key, else
/// `$XDG_MUSIC_DIR/gameboy`, else `~/Music/gameboy`, else a temp dir.
///
/// Never beside the ROM. An export belongs in the user's music folder; writing
/// it next to a cartridge is how a directory of games turns into a directory of
/// games and mystery files.
#[cfg(feature = "music")]
fn music_dir(config: &Config) -> PathBuf {
    if let Some(d) = config.music_dir.as_ref() {
        return PathBuf::from(expand_home(d));
    }
    let base = env::var_os("XDG_MUSIC_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join("Music")))
        .unwrap_or_else(env::temp_dir);
    // Same rule as screenshots and recordings: the new name, unless the old
    // one is what already exists on disk. See `config::app_dir_in`.
    crate::screen::config::app_dir_in(&base)
}

/// Load a cartridge **for listening**: no save file, no plugins.
///
/// Two deliberate differences from [`load_session`], and both are the point:
///
/// * `Gameboy::new(data, None)` — **music mode never touches a save.** The
///   mapper still comes from header byte `0x147` (`get_mbc` takes the path only
///   for persistence), so nothing about the machine is less accurate; there is
///   simply nowhere for cartridge RAM to be written. Browsing a shelf must not
///   be able to overwrite somebody's Pokémon save with the pristine RAM of a
///   thirty-second listen.
/// * No plugins. A trainer panel or a link partner attaching itself because you
///   opened a music player would be a surprise, and nothing here needs one.
#[cfg(feature = "music")]
#[cfg_attr(not(feature = "audio"), allow(clippy::let_unit_value))]
fn load_music_session(
    path: &Path,
    cfg: &crate::screen::config::Config,
) -> Result<(Gameboy, RomEntry, AudioHold), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if data.len() < 0x150 {
        return Err("file is too small to be a Game Boy ROM".to_string());
    }
    let entry = romlib::entry_from_bytes(path, &data);
    let mut gb = Gameboy::try_new_with_model(data, None, crate::mode::Model::Auto)
        .map_err(|e| e.to_string())?;
    if std::env::var_os(crate::gpu::PPU_ENV).is_none() {
        if let Some(mode) = cfg.ppu {
            gb.set_render_mode(mode);
        }
    }
    let audio = init_audio(&mut gb);
    Ok((gb, entry, audio))
}

#[cfg(feature = "music")]
impl App {
    /// Open the shelf. Scanning is instant (headers only); the covers and the
    /// verdicts arrive from a worker thread, so the library is browsable from
    /// the first frame.
    fn enter_music(&mut self) {
        let dir = music_dir(&self.config);
        let seconds = self.config.music_export_seconds;
        let opts = crate::music::library::ProbeOptions::default();
        let lib = crate::music::Library::scan(&self.config.rom_dir, opts);
        let empty = lib.is_empty();
        let mut state = mv::MusicState::new(lib, dir, seconds);
        state.image = self.new_cover_image();
        self.music = Some(state);
        self.screen = Screen::Shelf;
        self.overlay = Overlay::None;
        self.full_redraw = true;
        set_window_title(Some("Music"));
        if empty {
            self.set_toast(format!(
                "No .gb/.gbc ROMs in {} — set rom_dir in your config",
                self.config.rom_dir.display()
            ));
        }
        // Nothing is started here on purpose. An album's music begins at the
        // moment its cover was taken (see `Album::start`), and that moment is
        // not known until the worker has listened to it — a second or two. So
        // the needle drops in `autoplay_when_ready` instead, rather than
        // starting at the publisher logo and jumping a second later.
    }

    /// Leave music mode, stopping the music and putting the cover away.
    fn leave_music(&mut self) {
        self.erase_cover();
        self.music = None;
        self.session = None;
        self.screen = Screen::Picker;
        self.overlay = Overlay::None;
        self.full_redraw = true;
        set_window_title(None);
        self.recent = Recent::load();
        self.rescan();
    }

    /// The cover's terminal image — a **fourth** id, distinct from the game
    /// picture and the two trainer panels, for the reason in
    /// [`App::new_sprite_image`]: Kitty stores images by id and sharing one
    /// makes two pictures overwrite each other.
    fn new_cover_image(&self) -> Option<TermImage> {
        self.new_term_image()
            .map(|t| TermImage::new(t.wire(), 0x0067_6204))
    }

    /// Take the cover off the terminal.
    ///
    /// A Kitty placement lives until it is **deleted** — redrawing the cells
    /// under it does not remove it — and the image is also *invalidated* so the
    /// next placement re-transmits. Without that second half a still cover
    /// coming back after an overlay would deduplicate to "unchanged" and never
    /// reappear.
    fn erase_cover(&mut self) {
        let Some(m) = self.music.as_mut() else {
            return;
        };
        if let Some(img) = m.image.as_mut() {
            img.invalidate();
            if m.cover_shown {
                let seq = img.erase().to_string();
                m.pending_wire.push_str(&seq);
            }
        }
        if m.cover_shown {
            self.full_redraw = true;
        }
        m.cover_shown = false;
    }

    /// Drop the needle on the selected album as soon as the worker has listened
    /// to it, and not before.
    ///
    /// The alternative — start at reset and seek when the probe lands — was
    /// tried and is worse: the first seconds of most cartridges are a silent
    /// publisher logo, so it buys no music and then jumps. Waiting costs about
    /// two seconds, and the header says `listening…` while it does.
    fn autoplay_when_ready(&mut self) -> bool {
        let Some(m) = self.music.as_ref() else {
            return false;
        };
        if m.playing.is_some() || self.session.is_some() {
            return false;
        }
        let sel = m.sel;
        if !m.lib.get(sel).map(|a| a.probed).unwrap_or(false) {
            return false;
        }
        self.play_album(sel);
        true
    }

    /// Put album `idx` on: build a listening machine for it and start it.
    fn play_album(&mut self, idx: usize) {
        let Some((path, start)) = self
            .music
            .as_ref()
            .and_then(|m| m.lib.get(idx))
            .map(|a| (a.entry.path.clone(), a.start()))
        else {
            return;
        };
        let term_image = self.new_term_image();
        let sprite_image = self.new_sprite_image();
        let team_image = self.new_team_image();
        match load_music_session(&path, &self.config) {
            Ok((mut gb, entry, audio)) => {
                let title = entry.title.clone();
                // Skip the boot and start where the cover was taken. Without
                // this, selecting Pokémon Gold shows its title screen and plays
                // a minute of intro underneath it. A state that will not load
                // (a cartridge changed on disk since the probe) is not fatal —
                // the album simply starts from reset, which is still music.
                if let Some(bytes) = start.state.as_deref() {
                    let _ = gb.load_state(bytes);
                }
                #[cfg(feature = "audio")]
                let (audio_queue, audio_rate) = (audio.queue.clone(), audio.rate);
                self.session = Some(Session::new(
                    gb,
                    entry,
                    // No link partner and no cable: a music player has neither,
                    // and attaching one would be a surprise nobody asked for.
                    None,
                    None,
                    term_image,
                    sprite_image,
                    team_image,
                    #[cfg(feature = "audio")]
                    audio_queue,
                    #[cfg(feature = "audio")]
                    audio_rate,
                    #[cfg(feature = "audio")]
                    audio.stream,
                    #[cfg(not(feature = "audio"))]
                    audio,
                ));
                if let Some(m) = self.music.as_mut() {
                    m.playing = Some(idx);
                    m.started = Some(Instant::now());
                    m.play_offset = start.seconds();
                }
                set_window_title(Some(&format!("Music · {title}")));
            }
            Err(e) => {
                self.session = None;
                if let Some(m) = self.music.as_mut() {
                    m.playing = None;
                    m.started = None;
                    m.notice = Some(format!("cannot play {}: {e}", path.display()));
                }
            }
        }
    }

    /// Render `music_export_seconds` of the selected album's own audio to a WAV,
    /// **on its own thread**.
    ///
    /// A minute of audio renders in about a second of CPU. Doing that on the
    /// loop thread would drain the audio queue's ~41 ms cushion and be heard as
    /// a gap — the same rule as everywhere else in this frontend: nothing
    /// blocks the machine. See `docs/audio.md` §4c.
    fn export_selected(&mut self) {
        let Some(m) = self.music.as_mut() else {
            return;
        };
        if m.exporting.is_some() {
            m.notice = Some("still rendering the last one…".to_string());
            return;
        }
        let Some(album) = m.lib.get(m.sel) else {
            return;
        };
        if album.verdict == crate::music::AudioVerdict::Failed {
            m.notice = Some("this cartridge does not boot — nothing to save".to_string());
            return;
        }
        let title = album.entry.title.clone();
        let path = album.entry.path.clone();
        // The take starts where the cover does — see `Album::start`.
        let start = album.start().state;
        let seconds = m.export_seconds;
        let dest = crate::music::library::unique_export_path(
            &m.export_dir,
            &crate::music::library::export_name(&title, seconds),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let out = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|rom| {
                    crate::music::library::export_wav(
                        &rom,
                        &dest,
                        seconds,
                        start.as_deref(),
                    )
                });
            let _ = tx.send(out);
        });
        m.export_rx = Some(rx);
        m.exporting = Some(title.clone());
        m.notice = Some(format!("saving {seconds:.0}s of {title}…"));
    }

    /// Collect a finished export. Never blocks. Returns `true` if anything
    /// changed on screen.
    fn collect_export(&mut self) -> bool {
        let Some(m) = self.music.as_mut() else {
            return false;
        };
        let Some(rx) = m.export_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                m.export_rx = None;
                m.exporting = None;
                m.notice = Some(match result {
                    Ok(ex) => {
                        let kib = ex.bytes / 1024;
                        match ex.verdict {
                            crate::music::AudioVerdict::Audible => format!(
                                "saved {} ({}s, {kib} KiB)",
                                ex.path.display(),
                                ex.seconds.round()
                            ),
                            // A WAV of silence is a real file and a real
                            // disappointment; say so rather than reporting
                            // success and letting the player find out.
                            other => format!(
                                "saved {} — but {} ({}s)",
                                ex.path.display(),
                                other.label(),
                                ex.seconds.round()
                            ),
                        }
                    }
                    Err(e) => format!("save failed: {e}"),
                });
                let notice = m.notice.clone();
                if let Some(n) = notice {
                    self.set_toast(n);
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                m.export_rx = None;
                m.exporting = None;
                m.notice = Some("save failed: the renderer stopped".to_string());
                true
            }
        }
    }

    fn handle_shelf_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.leave_music(),
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                self.full_redraw = true;
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                if self.music.as_mut().map(|m| m.move_selection(-1)) == Some(true) {
                    self.on_selection_moved();
                }
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') | KeyCode::Char('j') => {
                if self.music.as_mut().map(|m| m.move_selection(1)) == Some(true) {
                    self.on_selection_moved();
                }
            }
            KeyCode::Home => {
                if self
                    .music
                    .as_mut()
                    .map(|m| m.move_selection(isize::MIN / 2))
                    == Some(true)
                {
                    self.on_selection_moved();
                }
            }
            KeyCode::End => {
                if self
                    .music
                    .as_mut()
                    .map(|m| m.move_selection(isize::MAX / 2))
                    == Some(true)
                {
                    self.on_selection_moved();
                }
            }
            KeyCode::Enter => {
                if let Some(sel) = self.music.as_ref().map(|m| m.sel) {
                    self.erase_cover();
                    self.play_album(sel);
                }
            }
            // Play it properly: leave the shelf and start the game, with a save
            // file this time. The library is a place to browse from.
            KeyCode::Char('g') => {
                let path = self
                    .music
                    .as_ref()
                    .and_then(|m| m.lib.get(m.sel))
                    .map(|a| a.entry.path.clone());
                if let Some(p) = path {
                    self.erase_cover();
                    self.music = None;
                    self.session = None;
                    self.full_redraw = true;
                    self.start_game(&p);
                }
            }
            KeyCode::Char(' ') => {
                if let Some(s) = self.session.as_mut() {
                    s.paused = !s.paused;
                    let paused = s.paused;
                    self.set_toast(if paused { "paused" } else { "playing" });
                }
            }
            KeyCode::Char('c') => {
                let (n, sel, playing) = match self.music.as_mut() {
                    Some(m) => {
                        let sel = m.sel;
                        let playing = m.selection_is_playing();
                        let n = match m.lib.get_mut(sel) {
                            Some(a) => {
                                a.cycle_cover();
                                a.covers.len()
                            }
                            None => 0,
                        };
                        (n, sel, playing)
                    }
                    None => (0, 0, false),
                };
                if n <= 1 {
                    self.set_toast("only one cover for this album");
                } else {
                    self.on_selection_moved();
                    // A candidate is a *moment*, not just a picture: it carries
                    // where the music starts. Choosing a different sleeve
                    // therefore moves the needle too, or the panel would claim
                    // a start time the sound does not match.
                    if playing {
                        self.play_album(sel);
                    }
                }
            }
            KeyCode::Char('w') | KeyCode::Char('W') => self.export_selected(),
            KeyCode::Char('r') => {
                // Rescan: the worker for the old library is dropped with it.
                self.erase_cover();
                self.enter_music();
            }
            KeyCode::Char(',') => self.bump_volume(false),
            KeyCode::Char('.') => self.bump_volume(true),
            KeyCode::Char('m') => self.toggle_mute(),
            _ => {}
        }
    }

    /// Moving the cursor changes the picture, so the cover must be
    /// retransmitted; and a shelf that keeps playing while you browse is the
    /// whole feel of a library, so the music is deliberately *not* touched.
    fn on_selection_moved(&mut self) {
        if let Some(m) = self.music.as_mut() {
            if let Some(img) = m.image.as_mut() {
                img.invalidate();
            }
        }
    }

    /// `NR52`'s low nibble: which sound channels are on, right now, on the
    /// machine that is playing. Read from the emulator, so the meter is evidence
    /// rather than decoration.
    fn live_channels(&mut self) -> Option<u8> {
        let s = self.session.as_mut()?;
        Some(s.gb.peek(0xFF26))
    }

    fn render_shelf(&mut self, f: &mut Frame<'_>) {
        let area = f.area();
        self.stats_rect = None;
        self.trainer_rect = None;
        self.sprite_rect = None;
        self.agent_rect = None;

        // The toast takes its own row rather than sitting on the picture — a
        // terminal image owns whole cells.
        let mut body = area;
        if self.toast_text().is_some() && body.height > 2 {
            body.height -= 1;
        }
        let (font_w, font_h) = self.picker.font_size();
        let l = mv::layout(body, font_w, font_h);

        let (count, ready, total, sel) = match self.music.as_ref() {
            Some(m) => {
                let (d, t) = m.lib.progress();
                (m.lib.len(), d, t, m.sel)
            }
            None => (0, 0, 0, 0),
        };
        let header = if ready < total {
            format!(" MUSIC MODE · {count} albums · listening… {ready}/{total}")
        } else {
            format!(" MUSIC MODE · {count} albums")
        };
        f.render_widget(
            Paragraph::new(header).style(
                Style::default()
                    .fg(Color::Indexed(213))
                    .add_modifier(Modifier::BOLD),
            ),
            l.header,
        );

        let channels = self.live_channels();
        self.render_album_panel(f, l.panel, channels);
        self.render_shelf_strip(f, l.strip, sel);

        let footer = " ←/→ browse · ⏎ play · w save WAV · c cover · space pause · \
                      g play the game · ? help · q leave";
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate(footer, l.footer.width as usize),
                Style::default().fg(Color::DarkGray),
            ))),
            l.footer,
        );

        self.render_cover(f, l.cover);
        if let Some(t) = self.toast_text().map(str::to_string) {
            let row = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    truncate(&format!(" {t}"), area.width as usize),
                    Style::default().fg(Color::Yellow),
                ))),
                row,
            );
        }
    }

    /// The cover art: the still frame chosen from that cartridge's own boot.
    ///
    /// **Deliberately not the live picture**, though the machine is running
    /// right there and it was written that way first. A record sleeve does not
    /// animate; `c` would have nothing to cycle if the picture were live; and —
    /// the deciding reason — a title screen is not where a game *stays*.
    /// Pokémon Blue rolls into an attract demo after about twenty seconds, so a
    /// shelf showing the live frame quietly turns into gameplay footage of a
    /// game nobody is playing. `g` is how you get the live picture.
    fn render_cover(&mut self, f: &mut Frame<'_>, area: Rect) {
        let rect =
            mv::fit_cover(area, self.picker.font_size().0, self.picker.font_size().1);
        if rect.width == 0 || rect.height == 0 {
            self.erase_cover();
            return;
        }
        // **A modal overlay must be able to paint where the cover is.** The
        // cover is a *still*, so its placement is deduplicated to "unchanged"
        // every frame and its cells are therefore marked `skip` — which tells
        // ratatui's diff to leave them alone, and the help overlay then draws
        // only the part of itself that misses the picture. The game picture
        // never hit this because a running game changes every frame.
        //
        // So while a popup is up the cover is taken off the terminal entirely
        // and put back (re-transmitted, see `erase_cover`) when it closes.
        if self.overlay != Overlay::None {
            self.erase_cover();
            return;
        }
        let palette = self.palette;
        let raw = self
            .music
            .as_ref()
            .and_then(|m| m.lib.get(m.sel))
            .and_then(|a| a.cover())
            .map(|c| c.rgba.clone());
        // Palette-mapped through the session's reusable buffer when there is
        // one, so the shelf honours the same palette the game view does.
        let mapped: Option<Vec<u8>> = match (raw, self.session.as_mut()) {
            (Some(r), Some(s)) => Some(s.prep.map(&r, palette).to_vec()),
            (Some(r), None) => Some(r),
            (None, _) => None,
        };
        let Some(rgba) = mapped else {
            // No picture for this album. Say so in its own rectangle rather
            // than leaving a stale cover from the last one on screen.
            self.erase_cover();
            let msg = Paragraph::new(Line::from(Span::styled(
                "no cover",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(ratatui::layout::Alignment::Center);
            if rect.height > 0 {
                f.render_widget(
                    msg,
                    Rect::new(rect.x, rect.y + rect.height / 2, rect.width, 1),
                );
            }
            return;
        };

        let Some(m) = self.music.as_mut() else {
            return;
        };
        let mut wire = std::mem::take(&mut m.pending_wire);
        let drawn = place_panel_image(
            f,
            m.image.as_mut(),
            &rgba,
            (crate::gpu::WIDTH as u32, crate::gpu::HEIGHT as u32),
            rect,
            false,
            // Never "modal": the popup path returned above, so if we are here
            // there is nothing drawn on top of the cover.
            false,
            &mut wire,
        );
        m.pending_wire = wire;
        m.cover_shown = drawn.is_some();
    }

    fn render_album_panel(
        &mut self,
        f: &mut Frame<'_>,
        area: Rect,
        channels: Option<u8>,
    ) {
        if area.width < 4 || area.height < 2 {
            return;
        }
        let inner = Rect::new(
            area.x + 1,
            area.y,
            area.width.saturating_sub(2),
            area.height,
        );
        let w = inner.width as usize;
        let mut lines: Vec<Line> = Vec::new();

        let Some(m) = self.music.as_ref() else {
            return;
        };
        let playing = m.selection_is_playing();
        let Some(album) = m.selected() else {
            f.render_widget(
                Paragraph::new("Nothing on the shelf.").wrap(Wrap { trim: true }),
                inner,
            );
            return;
        };

        lines.push(Line::from(Span::styled(
            truncate(&album.entry.title, w),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            truncate(
                &format!("{} · {} KiB", album.entry.mbc, album.entry.size / 1024),
                w,
            ),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));

        for (i, s) in mv::source_lines(album, playing).into_iter().enumerate() {
            let style = match (i, album.verdict) {
                (0, crate::music::AudioVerdict::Audible) => {
                    Style::default().fg(Color::Indexed(120))
                }
                (0, crate::music::AudioVerdict::Silent) => {
                    Style::default().fg(Color::DarkGray)
                }
                (0, _) => Style::default().fg(Color::Red),
                _ if s.starts_with('▶') => Style::default()
                    .fg(Color::Indexed(213))
                    .add_modifier(Modifier::BOLD),
                _ => Style::default().fg(Color::DarkGray),
            };
            lines.push(Line::from(Span::styled(truncate(&s, w), style)));
        }

        if playing {
            let meter = mv::channel_meter(channels.unwrap_or(0));
            lines.push(Line::from(Span::styled(
                truncate(&format!("  {}  {} ch1-4", mv::mmss(m.elapsed()), meter), w),
                Style::default().fg(Color::Indexed(120)),
            )));
        }

        lines.push(Line::from(""));
        match album.covers.len() {
            0 => lines.push(Line::from(Span::styled(
                truncate("▦ no cover — the game drew nothing usable", w),
                Style::default().fg(Color::DarkGray),
            ))),
            n => {
                // The scene's *start*, not the frame the cover was taken from:
                // that is where the music begins, so it is the number that
                // agrees with the clock above.
                let at = mv::mmss(album.start().seconds());
                lines.push(Line::from(Span::styled(
                    truncate(
                        &format!(
                            "▦ cover {}/{n} · from {at} into the boot{}",
                            album.cover_idx + 1,
                            if n > 1 { " · c to change" } else { "" }
                        ),
                        w,
                    ),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        if let Some(n) = m.notice.as_ref() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                truncate(n, w),
                Style::default().fg(Color::Yellow),
            )));
        }

        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    /// The row of other albums. Scrolls around the selection so a long shelf
    /// never overflows its rectangle.
    fn render_shelf_strip(&mut self, f: &mut Frame<'_>, area: Rect, sel: usize) {
        if area.height < 2 || area.width < 4 {
            return;
        }
        let rule = Rect::new(area.x, area.y, area.width, 1);
        f.render_widget(
            Paragraph::new("─".repeat(area.width as usize))
                .style(Style::default().fg(Color::Indexed(238))),
            rule,
        );
        let Some(m) = self.music.as_ref() else {
            return;
        };
        const CELL: usize = 22;
        let (from, to) = mv::strip_window(m.lib.len(), sel, area.width, CELL);
        let mut spans: Vec<Span> = Vec::new();
        for i in from..to {
            let Some(a) = m.lib.get(i) else { continue };
            let mark = if m.playing == Some(i) { "▶ " } else { "  " };
            let text = format!(
                "{mark}{:<width$}",
                truncate(&a.entry.title, CELL - 4),
                width = CELL - 4
            );
            let style = if i == sel {
                Style::default()
                    .bg(Color::Indexed(24))
                    .add_modifier(Modifier::BOLD)
            } else if m.playing == Some(i) {
                Style::default().fg(Color::Indexed(120))
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(text, style));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
}

/// One cell of the team strip, in pixels. The largest Gen 1 picture is 56×56,
/// so every roster slot gets the same box whatever its occupant's size and the
/// row does not jitter as the trainer's team changes.
#[cfg_attr(not(feature = "plugin-trainer"), allow(dead_code))]
const TEAM_CELL_PX: usize = 56;

/// Alpha applied to a fainted team member, so it reads as *out* rather than as
/// missing. Both protocols alpha-blend, so this is a ghost over the panel
/// rather than a colour we had to guess at.
#[cfg_attr(not(feature = "plugin-trainer"), allow(dead_code))]
const FAINTED_ALPHA: u8 = 80;

/// Compose the opposing trainer's roster into **one** wide RGBA strip:
/// `n × 56` by `56`, each Pokémon bottom-centred in its own cell, background
/// transparent, fainted members ghosted, unreadable slots left empty.
///
/// One image rather than six is the whole design. Six would mean six terminal
/// image ids, six placements and six PNG encodes every time the strip changed,
/// in a panel that is 38 cells wide — where each would land under six cells
/// anyway. A single strip is transmitted once, scaled by the terminal to the
/// team rectangle, and deduplicated by the same whole-image hash the game
/// picture uses, so a roster that has not changed costs nothing at all.
///
/// Returns `None` when nothing could be drawn, which the caller reports as the
/// marker line rather than as an empty band.
#[cfg(feature = "plugin-trainer")]
fn compose_team_strip(
    plugin: &mut TrainerPlugin,
    team: &[terminalgb_plugin_trainer::TeamMon],
    palette: Palette,
    out: &mut Vec<u8>,
) -> Option<(u32, u32)> {
    if team.is_empty() {
        return None;
    }
    let w = team.len() * TEAM_CELL_PX;
    let h = TEAM_CELL_PX;
    out.clear();
    out.resize(w * h * 4, 0);
    let mut drew = false;
    for (slot, m) in team.iter().enumerate() {
        if !m.known || m.species == 0 {
            continue;
        }
        let Some(img) = plugin.sprite(m.species) else {
            continue;
        };
        let (iw, ih) = (img.width, img.height);
        if iw == 0 || ih == 0 || iw > TEAM_CELL_PX || ih > TEAM_CELL_PX {
            continue;
        }
        let bg = terminalgb_plugin_trainer::sprite::background_mask(&img.pixels, iw, ih);
        // Bottom-centred: Gen 1 art is drawn standing on the bottom of its own
        // box, so aligning the *feet* is what makes a row of different-sized
        // pictures look like a line-up rather than a scatter.
        let ox = slot * TEAM_CELL_PX + (TEAM_CELL_PX - iw) / 2;
        let oy = TEAM_CELL_PX - ih;
        let alpha = if m.is_fainted() { FAINTED_ALPHA } else { 255 };
        for y in 0..ih {
            for x in 0..iw {
                let src = y * iw + x;
                if bg.get(src).copied().unwrap_or(false) {
                    continue;
                }
                let [r, g, b] = palette.shade(img.pixels[src]);
                let dst = ((oy + y) * w + ox + x) * 4;
                out[dst..dst + 4].copy_from_slice(&[r, g, b, alpha]);
            }
        }
        drew = true;
    }
    if !drew {
        out.clear();
        return None;
    }
    Some((w as u32, h as u32))
}

/// Largest centred rectangle inside `area` whose footprint matches the game
/// image at the terminal's pixel-per-cell `font` size, plus the effective
/// scale factor. There is deliberately **no upper cap** — the image grows to
/// fill the whole panel and re-fits when the terminal is resized.
fn fit_metrics(
    area: Rect,
    img_w: u32,
    img_h: u32,
    font_w: u16,
    font_h: u16,
) -> (Rect, f32) {
    if area.width == 0 || area.height == 0 || font_w == 0 || font_h == 0 {
        return (area, 1.0);
    }
    // Available drawing surface in pixels.
    let avail_w = area.width as f64 * font_w as f64;
    let avail_h = area.height as f64 * font_h as f64;
    // Largest scale that fits both dimensions.
    let scale = (avail_w / img_w as f64)
        .min(avail_h / img_h as f64)
        .max(0.0);
    let px_w = img_w as f64 * scale;
    let px_h = img_h as f64 * scale;
    // Footprint in cells, clamped to the area.
    let cols = ((px_w / font_w as f64).round() as u16)
        .clamp(1, area.width)
        .max(1);
    let rows = ((px_h / font_h as f64).round() as u16)
        .clamp(1, area.height)
        .max(1);
    let x = area.x + (area.width - cols) / 2;
    let y = area.y + (area.height - rows) / 2;
    (Rect::new(x, y, cols, rows), scale as f32)
}

/// How many emulated frames are due at `now`, and the deadline to carry
/// forward. **This is what keeps the game running at true speed when the
/// terminal cannot keep up.**
///
/// The deadline is advanced by exactly one `interval` per frame run, so the
/// long-run rate is exact (resetting it to `now` would silently add every loop
/// iteration's overhead to the period — the bug that made the emulator, and the
/// audio clock slaved to it, run slow). Catch-up is capped at [`MAX_CATCHUP`]
/// frames; past that the machine has been stalled long enough that replaying
/// the backlog would fast-forward the game, so the deadline is resynced and the
/// missed frames are simply lost.
fn frames_due(now: Instant, next_frame: Instant, interval: Duration) -> (u32, Instant) {
    if now < next_frame || interval.is_zero() {
        return (0, next_frame);
    }
    let behind = now.duration_since(next_frame);
    let due = (behind.as_nanos() / interval.as_nanos()) as u32 + 1;
    if due > MAX_CATCHUP {
        return (MAX_CATCHUP, now + interval);
    }
    (due, next_frame + interval * due)
}

/// When the next draw may start, given that the last one *started* at `started`
/// and took `cost`.
///
/// Never faster than one panel refresh (there would be no new picture), and
/// otherwise as often as the draw can sustain: a draw costing 50 ms yields a
/// 20 fps *display* while emulation carries on at 59.7275 fps in the gaps.
fn next_draw_at(started: Instant, cost: Duration) -> Instant {
    started + cost.max(FRAME_TIME)
}

/// A centred popup covering `pct_w`% × `pct_h`% of `area`.
fn popup_area(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = (area.width as u32 * pct_w as u32 / 100).max(10) as u16;
    let h = (area.height as u32 * pct_h as u32 / 100).max(5) as u16;
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect::new(x, y, w, h)
}

/// Order a scanned ROM list so recently-played entries (that still exist) come
/// first, and return a parallel "is recent" flag vector.
fn order_with_recent(
    scanned: Vec<RomEntry>,
    recent_paths: &[PathBuf],
) -> (Vec<RomEntry>, Vec<bool>) {
    // Canonicalise for comparison where possible.
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let recent_canon: Vec<PathBuf> = recent_paths.iter().map(|p| canon(p)).collect();

    let mut recent_entries = Vec::new();
    let mut rest = Vec::new();
    for e in scanned {
        let ec = canon(&e.path);
        match recent_canon.iter().position(|r| r == &ec) {
            Some(pos) => recent_entries.push((pos, e)),
            None => rest.push(e),
        }
    }
    recent_entries.sort_by_key(|(pos, _)| *pos);

    let mut entries: Vec<RomEntry> =
        Vec::with_capacity(recent_entries.len() + rest.len());
    let mut flags = Vec::with_capacity(entries.capacity());
    for (_, e) in recent_entries {
        entries.push(e);
        flags.push(true);
    }
    for e in rest {
        entries.push(e);
        flags.push(false);
    }
    (entries, flags)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Per-frame transmitted bytes, in units a human can act on.
///
/// This used to be `{:.0}K`, which renders the shared-memory transport's ~87 bytes
/// as `0K` and the compressed one's 2 KB as `2K` — i.e. it hid the difference
/// between "excellent" and "broken". The status panel is how you check whether the
/// transport is doing its job, so it has to show the small numbers.
fn human_bytes_f(n: f32) -> String {
    if n < 1024.0 {
        format!("{n:.0}B")
    } else if n < 100.0 * 1024.0 {
        format!("{:.1}K", n / 1024.0)
    } else {
        format!("{:.0}K", n / 1024.0)
    }
}

fn kv<'a>(key: &'a str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<8}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_string()),
    ])
}

// ---------------------------------------------------------------------------
// What actually went to the terminal
// ---------------------------------------------------------------------------

/// Bytes `ratatui` has written to the terminal, ever.
///
/// The cell-addressed protocols know their own payload size because *we* build
/// it ([`TermImage`]), but Sixel and half-blocks are drawn by `ratatui-image`
/// through the buffer, so their bytes only exist inside the backend's writer.
/// The status panel used to report a hard-coded `0 B` for those two — a number
/// that reads as "this transport is free" while 125 KB/s crosses the wire.
/// Counting at the writer is the one place that is true for every protocol.
static WROTE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn wrote_total() -> u64 {
    WROTE.load(Ordering::Relaxed)
}

/// A `Write` that counts. One relaxed add per `write`, on a path that is
/// already making a syscall.
struct CountingWriter<W: io::Write>(W);

impl<W: io::Write> io::Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.0.write(buf)?;
        WROTE.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

// ---------------------------------------------------------------------------
// Shared-memory capability
// ---------------------------------------------------------------------------

/// Whether this terminal answered "yes, I can read a frame out of shared
/// memory". Resolved once by [`probe_shm_support`] before the event loop starts.
static SHM_OK: AtomicBool = AtomicBool::new(false);

fn shm_supported() -> bool {
    SHM_OK.load(Ordering::Relaxed)
}

/// Whether this terminal answers `CSI 6 n`, and how long a bare round trip
/// takes — the two things [`crate::screen::termcost`] needs to be able to
/// measure the terminal's own per-frame cost. Also resolved by
/// [`probe_shm_support`], which already has `/dev/tty` open and has already
/// established that the terminal is talking.
static CPR_OK: AtomicBool = AtomicBool::new(false);
static CPR_FLOOR_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What the terminal answered about graphics, packed into one word so
/// [`build_picker`] can read it without a lock. `0` means "never asked" — which
/// is not the same as "answered, supports nothing", and must not be.
///
/// Layout: bit 0 `answered`, bit 1 kitty, bit 2 sixel, bits 8..24 cell width,
/// bits 24..40 cell height. A cell of `0` means the terminal reported none.
static GFX_CAPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pack_caps(c: termcaps::TermCaps) -> u64 {
    let (w, h) = c.cell.unwrap_or((0, 0));
    (c.answered as u64)
        | (c.kitty as u64) << 1
        | (c.sixel as u64) << 2
        | (w as u64) << 8
        | (h as u64) << 24
}

fn probed_caps() -> termcaps::TermCaps {
    let v = GFX_CAPS.load(Ordering::Relaxed);
    let (w, h) = (((v >> 8) & 0xffff) as u16, ((v >> 24) & 0xffff) as u16);
    termcaps::TermCaps {
        answered: v & 1 != 0,
        kitty: v & 2 != 0,
        sixel: v & 4 != 0,
        cell: (w > 0 && h > 0).then_some((w, h)),
    }
}

/// Ask the terminal which graphics protocol it speaks, and how big a cell is.
///
/// Writes one query and reads up to the cursor-position fence it ends with, so
/// this costs one round trip on top of the handshake the frontend already does.
/// Everything it learns is advisory: a terminal that says nothing leaves
/// [`build_picker`] exactly where it was.
#[cfg(all(unix, feature = "libc"))]
fn probe_graphics(tty: &mut std::fs::File) {
    use std::io::Write as _;
    let is_tmux = env::var("TERM").is_ok_and(|t| t.starts_with("tmux"))
        || env::var("TERM_PROGRAM").as_deref() == Ok("tmux");
    let q = termcaps::query(is_tmux);
    if tty
        .write_all(q.as_bytes())
        .and_then(|_| tty.flush())
        .is_err()
    {
        debug_layout("[gfx] could not write the capability query to /dev/tty");
        return;
    }
    let reply = read_reply(tty, SHM_PROBE_TIMEOUT);
    let caps = termcaps::parse(&reply);
    GFX_CAPS.store(pack_caps(caps), Ordering::Relaxed);
    // The protocol is negotiated once and invisibly. When it comes out worse
    // than expected the only way to tell a terminal that genuinely cannot do
    // better from a reply we mis-read is to see the bytes.
    debug_layout(&format!(
        "[gfx] kitty={} sixel={} cell={:?} answered={} reply={reply:?}",
        caps.kitty, caps.sixel, caps.cell, caps.answered
    ));
}

#[cfg(not(all(unix, feature = "libc")))]
fn probe_graphics(_tty: &mut std::fs::File) {}

/// The measured cost model for this terminal, or an unmeasurable one.
///
/// `GB_TERM_FENCE=off` opts out: the frontend then paces at the display rate as
/// it always did, and says `n/a` rather than showing a ceiling it did not
/// measure.
fn terminal_cost_model() -> termcost::TerminalCost {
    if env::var("GB_TERM_FENCE").as_deref() == Ok("off") {
        return termcost::TerminalCost::unmeasurable();
    }
    termcost::TerminalCost::new(
        CPR_OK.load(Ordering::Relaxed),
        Duration::from_micros(CPR_FLOOR_US.load(Ordering::Relaxed)),
    )
}

/// Time one round trip to the terminal, with whatever we last wrote queued
/// ahead of it.
///
/// A cursor-position report is a **fence**: the terminal answers it only after
/// working through everything written before it, so with a picture just written
/// this measures that picture's cost *to the terminal* — the half of the frame
/// our own timers cannot see.
///
/// `crossterm::cursor::position()` is the only safe way to do this once the
/// event loop is running. It reads through crossterm's own event source, so a
/// keypress arriving during the wait is queued and delivered afterwards; reading
/// `/dev/tty` directly here (which is right at *startup*, before the loop owns
/// stdin — see [`probe_terminal`]) would race crossterm for the same input and
/// silently eat keys.
fn fence() -> Option<Duration> {
    let t = Instant::now();
    match ratatui::crossterm::cursor::position() {
        Ok(_) => Some(t.elapsed()),
        Err(_) => None,
    }
}

/// Time a bare cursor-position round trip a few times and keep the fastest.
///
/// This is the *fixed* part of a fence — pty latency plus the terminal's input
/// loop — with nothing queued for it to work through. It is subtracted from
/// every later sample so that what is reported is the terminal's frame work and
/// not our round-trip overhead. Measured on WezTerm it is 3.35 ms at 80×24 and
/// 3.39 ms at 640×176: flat across a 65× change in composited area, which is
/// what a fixed latency looks like and why subtracting a constant is right.
fn measure_cpr_floor(tty: &mut std::fs::File) {
    use std::io::Write as _;
    let mut best: Option<Duration> = None;
    for _ in 0..termcost::SAMPLES {
        let t = Instant::now();
        if tty.write_all(b"\x1b[6n").and_then(|_| tty.flush()).is_err() {
            return;
        }
        if !read_reply(tty, HANDSHAKE_KNOCK_WAIT).contains('R') {
            return;
        }
        let d = t.elapsed();
        best = Some(best.map_or(d, |b: Duration| b.min(d)));
    }
    if let Some(b) = best {
        CPR_OK.store(true, Ordering::Relaxed);
        CPR_FLOOR_US.store(b.as_micros() as u64, Ordering::Relaxed);
        debug_layout(&format!("[fence] floor {:.3} ms", b.as_secs_f64() * 1000.0));
    }
}

/// Ask the terminal what it can do, once, before the event loop owns stdin.
///
/// Two answers come out of this: whether it implements Kitty's `t=s`
/// transmission medium, and whether it answers `CSI 6 n` (plus how fast) — the
/// latter being what lets [`crate::screen::termcost`] measure the terminal's own
/// per-frame cost instead of the frontend reporting a number that can only ever
/// read 59.
///
/// **Ask; do not infer.** WezTerm implements the Kitty graphics protocol but
/// not its Unicode placeholders, so any rule of the form "if `$TERM` looks like
/// X then it supports Y" is wrong in a way that shows the user a blank
/// rectangle. The protocol has an `a=q` handshake precisely for this; a
/// cursor-position request is appended as a fence so that *silence* is a
/// conclusive "no" rather than us not having waited long enough.
///
/// Failure in any direction — no reply, an error reply, no `/dev/shm`, a
/// platform without shared memory — leaves the flag false and the frontend on
/// the compressed-PNG transport, which works everywhere.
fn probe_terminal() {
    if !cfg!(unix) {
        return;
    }
    // Talk to `/dev/tty`, not to stdout/stdin.
    //
    // Writing the query to `io::stdout()` and reading the answer from fd 0
    // measurably does not work here: `crossterm` owns those handles (raw mode,
    // its own terminal queries during startup, its buffered event source), and
    // the reply never reaches our read — measured as an empty reply against a
    // WezTerm that answers the same query on `/dev/tty` in single-digit
    // milliseconds. The controlling terminal is the same device either way, so
    // opening it directly costs nothing and shares nothing.
    let Some(mut tty) = open_tty() else {
        debug_layout("[shm] no /dev/tty — assuming no shared memory");
        return;
    };
    use std::io::Write as _;
    // Wait for the terminal to be *demonstrably answering* before asking it
    // anything that matters.
    //
    // A terminal launched for this process is still starting up when we get
    // here, and a capability question asked too early is answered with silence
    // — which is indistinguishable from "not supported" and costs the session
    // its best transport for the whole run. Measured on a cold WezTerm window:
    // asking immediately came back empty in most runs. So knock first with a
    // cursor-position request, which every terminal answers, and only ask once
    // it has.
    if !terminal_is_answering(&mut tty) {
        debug_layout("[shm] terminal never answered a cursor report — assuming no");
        return;
    }
    // It answers: time a bare round trip while we still own the terminal. This
    // is the floor under every later fence, and the loop cannot measure it
    // itself because by then there is always a picture queued behind it.
    measure_cpr_floor(&mut tty);

    // Which graphics protocol does it speak? Asked here, on the same open
    // `/dev/tty`, for the same reason the shared-memory question is: this is the
    // one moment where raw mode is on and nothing else is reading the terminal.
    // It must come *before* the shared-memory probe, which returns early on
    // `GB_TERM_KITTY=noshm` and when there is no shared memory to offer.
    probe_graphics(&mut tty);

    if std::env::var("GB_TERM_KITTY").as_deref() == Ok("noshm") {
        return;
    }
    let Some(query) = crate::termimage::shm_query(SHM_QUERY_ID) else {
        debug_layout("[shm] no shared memory available to offer the terminal");
        return;
    };
    let asked = tty
        .write_all(query.seq.as_bytes())
        .and_then(|_| tty.write_all(b"\x1b[6n"))
        .and_then(|_| tty.flush())
        .is_ok();
    if asked {
        let reply = read_reply(&tty, SHM_PROBE_TIMEOUT);
        let ok = crate::termimage::shm_reply_ok(&reply, SHM_QUERY_ID);
        SHM_OK.store(ok, Ordering::Relaxed);
        // The transport is negotiated once and invisibly; when it comes out
        // "no" the only way to tell a terminal that refused from a reply we
        // mis-read is to see the bytes. Gated, so it costs nothing normally.
        debug_layout(&format!(
            "[shm] asked id={SHM_QUERY_ID} ok={ok} reply={:?}",
            reply
        ));
    } else {
        debug_layout("[shm] could not write the query to /dev/tty");
    }
    query.finish();
}

const SHM_QUERY_ID: u32 = 0x0067_6209;

/// Knock until the terminal answers a cursor-position request, or give up.
///
/// Returns as soon as one reply arrives, so on a warm terminal this costs a
/// millisecond or two. The total budget is deliberately long because the cost
/// of being impatient is silent and lasts the whole session, while the cost of
/// waiting is paid once, at launch, only by a terminal that is not answering.
fn terminal_is_answering(tty: &mut std::fs::File) -> bool {
    use std::io::Write as _;
    // Discard anything already queued. `crossterm` queries the terminal during
    // startup (keyboard-enhancement support), and a reply of its that is still
    // sitting in the buffer would be read as the answer to *our* question —
    // which then reports "no shared memory" while the real reply arrives a
    // moment later and is thrown away. Observed as an intermittent false
    // negative on cold starts.
    while !read_reply(tty, Duration::from_millis(20)).is_empty() {}
    for _ in 0..HANDSHAKE_KNOCKS {
        if tty.write_all(b"\x1b[6n").and_then(|_| tty.flush()).is_err() {
            return false;
        }
        if read_reply(tty, HANDSHAKE_KNOCK_WAIT).contains('R') {
            return true;
        }
    }
    false
}

const HANDSHAKE_KNOCKS: usize = 10;
const HANDSHAKE_KNOCK_WAIT: Duration = Duration::from_millis(300);

/// How long to wait for the terminal's capability answer at startup.
///
/// Generous on purpose, and it almost never costs anything: the wait ends the
/// moment the cursor-position **fence** arrives, which a live terminal answers
/// in single-digit milliseconds, so this deadline is only reached by a terminal
/// that is not answering at all.
///
/// It has to be generous because a terminal launched *for* this process is
/// still starting up when we ask. Measured on a cold WezTerm window: at 500 ms
/// the probe came back completely empty — no reply, not even the fence — in two
/// runs out of three, and the session silently fell back to the PNG transport
/// for its whole lifetime. Getting this wrong is invisible except in the status
/// panel, which is exactly the kind of failure worth over-waiting to avoid.
const SHM_PROBE_TIMEOUT: Duration = Duration::from_millis(2500);

/// Read whatever the terminal says back, up to the cursor-position fence.
///
/// `crossterm` cannot help here: it parses input into `Event`s and drops the
/// application-specific `APC` reply we are waiting for. So this reads the
/// controlling terminal directly, and only during startup — once the event loop
/// owns the terminal, nothing else may read it.
#[cfg(all(unix, feature = "libc"))]
fn open_tty() -> Option<std::fs::File> {
    std::fs::File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()
}

#[cfg(not(all(unix, feature = "libc")))]
fn open_tty() -> Option<std::fs::File> {
    None
}

#[cfg(all(unix, feature = "libc"))]
fn read_reply(tty: &std::fs::File, timeout: Duration) -> String {
    use std::os::unix::io::AsRawFd;
    let fd = tty.as_raw_fd();
    let deadline = Instant::now() + timeout;
    let mut got = String::new();
    let mut buf = [0u8; 256];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut p, 1, left.as_millis() as i32) } <= 0 {
            break;
        }
        let n =
            unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        got.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
        if got.contains('R') {
            break; // the fence arrived: everything else is already here
        }
    }
    got
}

#[cfg(not(all(unix, feature = "libc")))]
fn read_reply(_tty: &std::fs::File, _timeout: Duration) -> String {
    String::new()
}

/// The cell size assumed when the terminal reports none.
///
/// Only reached by a terminal that answers neither `CSI 16 t` nor the
/// `CSI 14 t` / `CSI 18 t` pair *and* leaves `TIOCGWINSZ`'s pixel fields at
/// zero. It is a guess, and it is only ever a fit-and-aspect guess: no protocol
/// draws fewer pixels because of it.
const DEFAULT_CELL: (u16, u16) = (10, 20);

/// Build a [`Picker`] on the best graphics protocol the terminal actually
/// advertises.
///
/// # The ladder
///
/// **Kitty → Sixel → iTerm2 → half-blocks**, and half-blocks is what you get
/// when nothing better exists — never the default for "not kitty". Kitty leads
/// because it is cell-addressed *and* can take a frame through shared memory
/// (~15 bytes a frame); Sixel draws real pixels but re-transmits the whole
/// picture; iTerm2 is cell-addressed but cannot be probed at all, so it sits
/// below the two that can be; half-blocks is text — two pixels per character
/// cell, which is the coarse picture this ladder exists to avoid.
///
/// The first two rungs come from [`probe_graphics`], which asked the terminal on
/// `/dev/tty` before the event loop started. The third is an env-var guess that
/// [`Picker::from_fontsize`] makes for us (`TERM_PROGRAM` naming iTerm2, WezTerm,
/// mintty, …, plus tmux's outer-terminal hints), and it applies only when the
/// probe found nothing better — so an *advertised* protocol always beats a
/// guessed one, which is the property the old implementation went to the trouble
/// of suppressing `$TERM_PROGRAM` to get.
///
/// # Why not `Picker::from_query_stdio()`
///
/// Because it discards a good capability answer. It returns `Err(NoFontSize)`
/// when the terminal reports no cell size, and the caller is then left with
/// `Picker::from_fontsize`, whose protocol is `Halfblocks`. Measured on VTE
/// 0.84.1 (Ptyxis, Fedora 44): `CSI 16 t` is unanswered and `TIOCGWINSZ` reports
/// `xpixel=0 ypixel=0`, so both of the library's sources of a cell size fail and
/// the result is discarded. On VTE that happens to be harmless — it advertises
/// no graphics protocol either way (`DA1 = ?61;1;21;22;28`, no `4`; Fedora
/// builds VTE without Sixel, `vte_get_features()` = `+BIDI +GNUTLS +ICU
/// +SYSTEMD`) — but on any Sixel- or Kitty-capable terminal with the same gap it
/// silently costs the whole session its protocol. See
/// [`crate::screen::termcaps`], which also recovers the cell size from
/// `CSI 14 t` / `CSI 18 t`.
///
/// It also queries stdout/stdin, which `crossterm` owns; our probe uses
/// `/dev/tty`, drains stale input and knocks first, for the reasons documented
/// on [`probe_terminal`].
fn build_picker() -> Picker {
    let caps = probed_caps();
    // `from_fontsize` seeds the tmux flag and the iTerm2-from-env guess for us —
    // rung three of the ladder — so it is the right base whatever we then decide.
    let mut picker = Picker::from_fontsize(caps.cell.unwrap_or(DEFAULT_CELL));

    if let Some(best) = caps.best() {
        picker.set_protocol_type(match best {
            termcaps::Graphics::Kitty => ProtocolType::Kitty,
            termcaps::Graphics::Sixel => ProtocolType::Sixel,
            termcaps::Graphics::Iterm2 => ProtocolType::Iterm2,
            termcaps::Graphics::Halfblocks => ProtocolType::Halfblocks,
        });
        return picker;
    }

    // The terminal answered and advertised neither. Keep the env guess (iTerm2)
    // or half-blocks — both already set by `from_fontsize`.
    if caps.answered {
        return picker;
    }

    // Nobody answered: either the probe could not run (no `/dev/tty`, no `libc`
    // feature, not unix) or the terminal never replied. Fall back to the
    // library's own query rather than declaring half-blocks on no evidence.
    match Picker::from_query_stdio() {
        Ok(p) => p,
        Err(_) => picker,
    }
}

/// Resolve a [`CellScale`] to a concrete device-pixels-per-cell multiplier.
///
/// Priority (first hit wins):
///   1. `$TERMINALGB_CELL_SCALE` — a quick override for testing, beats the
///      config. `$GAMEBOY_CELL_SCALE` is the pre-rename spelling and is still
///      honoured, so anybody's existing shell alias keeps working.
///   2. an explicit `CellScale::Fixed(x)` from the config.
///   3. `CellScale::Auto` → [`detect_display_scale`].
///
/// Always returns a finite value in `[1.0, 8.0]`; `1.0` means "no correction".
fn resolve_cell_scale(configured: CellScale) -> f32 {
    let clamp = |v: f32| {
        if v.is_finite() {
            v.clamp(1.0, 8.0)
        } else {
            1.0
        }
    };
    if let Some(v) = env::var("TERMINALGB_CELL_SCALE")
        .or_else(|_| env::var("GAMEBOY_CELL_SCALE"))
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
    {
        return clamp(v);
    }
    match configured {
        CellScale::Fixed(x) => clamp(x),
        CellScale::Auto => clamp(detect_display_scale()),
    }
}

/// Best-effort detection of the display's pixel scale factor, for
/// `cell_scale = auto`. This is inherently heuristic — there is no in-band way
/// to read a Wayland output's scale from inside a terminal — so it is opt-in and
/// falls back to `1.0` (no correction) whenever nothing reliable is found.
///
/// Order: `$GDK_SCALE` (integer), `$QT_SCALE_FACTOR` (float), then the X
/// resource `Xft.dpi` (GNOME sets it for the whole session, XWayland included)
/// read via `xrdb`, as `dpi / 96`.
fn detect_display_scale() -> f32 {
    if let Some(v) = env::var("GDK_SCALE")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
    {
        if v.is_finite() && v > 0.0 {
            return v;
        }
    }
    if let Some(v) = env::var("QT_SCALE_FACTOR")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
    {
        if v.is_finite() && v > 0.0 {
            return v;
        }
    }
    if let Some(dpi) = xft_dpi() {
        if dpi > 0.0 {
            return dpi / 96.0;
        }
    }
    1.0
}

/// Read `Xft.dpi` from the X resource database via `xrdb -query`. Returns `None`
/// if `xrdb` is absent, errors, or the key is missing (e.g. a headless or
/// non-X session). One cheap sub-process at startup, never on the hot path.
fn xft_dpi() -> Option<f32> {
    let out = std::process::Command::new("xrdb")
        .arg("-query")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Xft.dpi:") {
            if let Ok(dpi) = rest.trim().parse::<f32>() {
                return Some(dpi);
            }
        }
    }
    None
}

fn protocol_from_str(s: &str) -> Option<ProtocolType> {
    match s.trim().to_ascii_lowercase().as_str() {
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        "halfblocks" | "blocks" | "ascii" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

fn protocol_name(p: ProtocolType) -> &'static str {
    match p {
        ProtocolType::Kitty => "kitty",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Iterm2 => "iterm2",
        ProtocolType::Halfblocks => "halfblocks",
    }
}

/// Map a [`KeypadKey`] to a stable 0..8 index (matches the enum order).
fn key_index(k: KeypadKey) -> usize {
    match k {
        KeypadKey::Right => 0,
        KeypadKey::Left => 1,
        KeypadKey::Up => 2,
        KeypadKey::Down => 3,
        KeypadKey::A => 4,
        KeypadKey::B => 5,
        KeypadKey::Select => 6,
        KeypadKey::Start => 7,
    }
}

fn key_from_index(i: usize) -> KeypadKey {
    match i {
        0 => KeypadKey::Right,
        1 => KeypadKey::Left,
        2 => KeypadKey::Up,
        3 => KeypadKey::Down,
        4 => KeypadKey::A,
        5 => KeypadKey::B,
        6 => KeypadKey::Select,
        _ => KeypadKey::Start,
    }
}

// ---------------------------------------------------------------------------
// ROM loading + framebuffer conversion
// ---------------------------------------------------------------------------

/// What keeps the game's host-audio alive, plus the read-only diagnostic handle
/// telemetry reports from. Zero-sized without the `audio` feature.
#[cfg(feature = "audio")]
struct AudioHold {
    stream: Option<cpal::Stream>,
    queue:
        Option<std::sync::Arc<std::sync::Mutex<crate::sound::sample_queue::SampleQueue>>>,
    rate: u32,
}
#[cfg(not(feature = "audio"))]
type AudioHold = ();

/// The value [`Session`] holds for the lifetime of the game: dropping it stops
/// the sound.
#[cfg(feature = "audio")]
type AudioStream = Option<cpal::Stream>;
#[cfg(not(feature = "audio"))]
type AudioStream = ();

/// Wire host audio into `gb`, returning the stream to keep alive. Feature-gated
/// (cpal pulls in ALSA) and graceful when no output device is available. Mirror
/// of `Gameboy::render_desktop`'s audio init, but per-session so it follows the
/// active game across in-app ROM switches. Deliberately silent (no `eprintln`)
/// because we are inside the alternate screen — a stray line would corrupt it.
#[cfg(feature = "audio")]
fn init_audio(gb: &mut Gameboy) -> AudioHold {
    use crate::sound::AudioPlayer;
    // A fresh attempt for this session: clear any state the previous ROM left.
    // Success/failure is published by `CpalPlayer::new` into
    // `sound::host_status`, which the Status panel's `Audio` row, the
    // entering-a-game toast and telemetry's `audio_host` all read — this path
    // cannot print, it is inside the alternate screen.
    crate::sound::host_status::reset();
    match crate::sound::cpal_audio::CpalPlayer::new() {
        Some((player, stream)) => {
            // Grab the diagnostic handles before the player is boxed away —
            // `sound.rs` already publishes both, so telemetry needs no change
            // there.
            let queue = Some(player.queue_handle());
            let rate = player.samples_rate();
            gb.enable_audio(Box::new(player));
            AudioHold {
                stream: Some(stream),
                queue,
                rate,
            }
        }
        None => AudioHold {
            stream: None,
            queue: None,
            rate: 0,
        },
    }
}
#[cfg(not(feature = "audio"))]
fn init_audio(_gb: &mut Gameboy) -> AudioHold {}

/// Set the terminal window's title to `TERM_TITLE`, plus the cartridge when
/// there is one.
///
/// Best-effort and unconditional: a terminal that does not understand OSC 2
/// ignores it, and one that does gains a window an OBS scene can find. Written
/// straight to stdout for the same reason the image sequences are — this runs
/// while `ratatui` owns the screen, and a title escape is not a cell.
fn set_window_title(game: Option<&str>) {
    let _ = execute!(io::stdout(), SetTitle(window_title(game)));
}

/// `TERM_TITLE`, plus the cartridge when there is a readable one. Split out so
/// the stable-prefix promise is testable without a terminal.
fn window_title(game: Option<&str>) -> String {
    match game.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => format!("{TERM_TITLE} — {g}"),
        None => TERM_TITLE.to_string(),
    }
}

/// Build a [`Gameboy`], its [`RomEntry`] and its audio hold from a ROM on disk.
///
/// NOTE: always pass `Some(path)` to `Gameboy::new` so the MBC is detected from
/// the header — with `None`, `get_mbc` forces MBC3 and mis-drives other carts.
// `audio` is `()` without the audio feature; that unit binding is intentional.
#[cfg_attr(not(feature = "audio"), allow(clippy::let_unit_value))]
fn load_session(
    path: &Path,
    cfg: &crate::screen::config::Config,
) -> Result<(Gameboy, RomEntry, AudioHold, Vec<String>), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if data.len() < 0x150 {
        return Err("file is too small to be a Game Boy ROM".to_string());
    }
    let entry = romlib::entry_from_bytes(path, &data);
    // An **optional** boot ROM the user dumped from their own hardware. Nothing
    // is bundled and nothing is fetched; with no file at the documented path
    // this is exactly `Gameboy::new`, which is what a fresh clone gets and what
    // every checked-in baseline is measured on. See [`crate::boot`].
    let model = crate::mode::Model::Auto;
    let (boot, boot_note) = crate::boot::explain(model.resolve(data[0x0143]));
    let booting = boot.is_some();
    let mut gb =
        Gameboy::new_with_boot_rom(data.clone(), Some(path.to_path_buf()), model, boot);
    // `$GB_PPU` has already chosen an engine inside `Gpu::new`; the config key
    // only speaks when the environment did not, so a one-off `GB_PPU=fast` run
    // is not silently undone by a config file.
    if std::env::var_os(crate::gpu::PPU_ENV).is_none() {
        if let Some(mode) = cfg.ppu {
            gb.set_render_mode(mode);
        }
    }
    let audio = init_audio(&mut gb);
    let mut notes = attach_plugins(&mut gb, cfg, &data);
    // Say it once, in the places `config_warnings` already reaches. A boot ROM
    // changes the machine — it is running somebody else's code before the
    // cartridge's — so it must never be invisible; and a file the user clearly
    // meant to supply that could not be used must say why rather than silently
    // doing nothing.
    if booting {
        notes.push("boot ROM: running the supplied image".to_string());
    }
    if let Some(why) = boot_note {
        notes.push(why);
    }
    Ok((gb, entry, audio, notes))
}

/// Attach the plugins the user configured, returning a line per plugin — one
/// that says what attached, or why one did not.
///
/// **Nothing is attached implicitly.** A plugin is loaded only because a name
/// appears in `plugins = …` (or `GB_PLUGINS`), and the result is reported so an
/// active plugin is never a surprise. See [`crate::plugin`].
///
/// This function is the **install path**, and it is deliberately the only one:
/// adding a plugin — ours or a stranger's — is a Cargo dependency plus one
/// entry in [`REGISTRY`] plus one arm in [`attach_named`]. See
/// [`docs/plugins.md`](../docs/plugins.md) §"Installing a plugin".
fn attach_plugins(
    gb: &mut Gameboy,
    cfg: &crate::screen::config::Config,
    rom: &[u8],
) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(msg) = gb.open_plugin_trace() {
        notes.push(msg);
    }
    // The trainer view is the one plugin attached without being asked for. That
    // is defensible precisely because it is *inert*: it declares no
    // WRITE_MEMORY, and it does no work at all until `T` switches its panel on.
    // Everything that can touch the machine still has to be named explicitly.
    #[cfg(feature = "plugin-trainer")]
    {
        let mut trainer = TrainerPlugin::new(rom.to_vec());
        trainer.set_active(false);
        if let Err(e) = gb.add_plugin(Box::new(trainer)) {
            notes.push(format!("plugin trainer: {e}"));
        }
    }
    // The environment wins over the config file, as it does for telemetry.
    let names: Vec<String> = match std::env::var("GB_PLUGINS") {
        Ok(v) => v
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => cfg.plugins.clone(),
    };
    for name in names {
        match attach_named(gb, cfg, rom, &name) {
            Some(note) => {
                if !note.is_empty() {
                    notes.push(note)
                }
            }
            // Told apart deliberately: "you have not built it in" and "that is
            // not a plugin" are different problems with different fixes, and a
            // single "unknown plugin" message sends the user hunting for a typo
            // that is not there.
            None => notes.push(match REGISTRY.iter().find(|(n, _)| *n == name) {
                Some((_, feature)) => format!(
                    "plugin {name} is not built into this binary — rebuild with \
                     --features {feature}"
                ),
                None => format!("unknown plugin '{name}'"),
            }),
        }
    }
    notes
}

/// Every plugin this project ships, whether or not this binary was built with
/// it: `(name, cargo feature)`.
///
/// The point of listing the ones that are *absent* is the error message — see
/// [`attach_plugins`]. A third-party plugin adds a row here only if it wants
/// the same courtesy; it works without one.
const REGISTRY: &[(&str, &str)] = &[
    ("trainer", "plugin-trainer"),
    ("gen1-link", "plugin-gen1-link"),
    ("gen1-battle", "plugin-battle-control"),
    ("agent", "plugin-agent"),
    ("gen1-shinyhunt", "plugin-gen1-shinyhunt"),
    ("example-tracer", "plugin-example-tracer"),
];

/// Build and attach one named plugin.
///
/// `None` means "this build does not have a plugin by that name" — the caller
/// turns that into the right message. `Some(note)` means the name was
/// recognised, whether attaching succeeded or failed; an empty note means
/// "recognised, nothing to report".
///
/// **This is the function a third-party plugin edits.** One `if`, gated on the
/// feature that brings the crate in, is the whole integration.
fn attach_named(
    gb: &mut Gameboy,
    cfg: &crate::screen::config::Config,
    rom: &[u8],
    name: &str,
) -> Option<String> {
    // Already attached above; naming it explicitly is not an error.
    #[cfg(feature = "plugin-trainer")]
    if name == terminalgb_plugin_trainer::NAME {
        return Some(String::new());
    }

    #[cfg(feature = "plugin-gen1-link")]
    if name == terminalgb_plugin_gen1_link::NAME {
        let trade = std::env::var("GB_GEN1_TRADE")
            .ok()
            .or_else(|| cfg.gen1_trade.clone());
        let path = trade.as_deref().map(std::path::Path::new);
        return Some(
            match terminalgb_plugin_gen1_link::Gen1LinkPartner::from_trade_file(
                rom.to_vec(),
                path,
            ) {
                Ok(p) => {
                    let offer = p.offer().to_string();
                    match gb.add_plugin(Box::new(p)) {
                        Ok(()) => {
                            format!("plugin gen1-link attached, offering {offer}")
                        }
                        Err(e) => format!("plugin gen1-link: {e}"),
                    }
                }
                Err(e) => format!("plugin gen1-link: {e}"),
            },
        );
    }

    #[cfg(feature = "plugin-battle-control")]
    if name == terminalgb_plugin_battle_control::NAME {
        let file = std::env::var("GB_BATTLES")
            .ok()
            .or_else(|| cfg.battles.clone());
        let path = file.as_deref().map(std::path::Path::new);
        return Some(
            match terminalgb_plugin_battle_control::BattlePlugin::from_preset_file(
                rom.to_vec(),
                path,
            ) {
                Ok(p) => {
                    let n = p.preset_names().len();
                    match gb.add_plugin(Box::new(p)) {
                        Ok(()) => format!(
                            "plugin gen1-battle attached, {n} presets \
                             (press B to start one, N to choose)"
                        ),
                        Err(e) => format!("plugin gen1-battle: {e}"),
                    }
                }
                Err(e) => format!("plugin gen1-battle: {e}"),
            },
        );
    }

    #[cfg(feature = "plugin-agent")]
    if name == terminalgb_plugin_agent::NAME {
        return Some(match build_agent(cfg) {
            Ok(p) => {
                let driver = p.driver_name().to_string();
                match gb.add_plugin(Box::new(p)) {
                    Ok(()) => format!(
                        "plugin agent attached, driven by {driver} — it \
                         presses the buttons, and `G` takes the controller back"
                    ),
                    Err(e) => format!("plugin agent: {e}"),
                }
            }
            Err(e) => format!("plugin agent: {e}"),
        });
    }

    // The hunter, attached to the *interactive* machine, is a watcher: it
    // reports whether the wild Pokémon on screen matches the target, in the
    // status panel, without touching anything (it declares no WRITE_MEMORY and
    // no CONTROL_INPUT). The parallel hunt is a different program —
    // `examples/shiny_hunt.rs` — because N machines and a driving loop are not
    // things a plugin attached to somebody's game can be.
    #[cfg(feature = "plugin-gen1-shinyhunt")]
    if name == terminalgb_plugin_gen1_shinyhunt::NAME {
        let spec = std::env::var("GB_SHINY_TARGET").unwrap_or_else(|_| "shiny".into());
        return Some(
            match terminalgb_plugin_gen1_shinyhunt::Target::parse(&spec, None) {
                Ok(target) => {
                    let label = target.label();
                    let odds = target.expected_encounters();
                    let p = terminalgb_plugin_gen1_shinyhunt::ShinyHunt::new(target);
                    match gb.add_plugin(Box::new(p)) {
                        Ok(()) => format!(
                            "plugin gen1-shinyhunt attached, watching for {label} \
                             (1 in {odds:.0} encounters)"
                        ),
                        Err(e) => format!("plugin gen1-shinyhunt: {e}"),
                    }
                }
                Err(e) => format!("plugin gen1-shinyhunt: {e}"),
            },
        );
    }

    // The documentation example, and the shape every third-party arm takes:
    // one `if`, gated on the feature that brings the crate in. `GB_TRACE_ADDR`
    // is this plugin's own configuration; a real plugin would more likely read
    // a key from `config.toml`, as gen1-link does above.
    #[cfg(feature = "plugin-example-tracer")]
    if name == terminalgb_plugin_example_tracer::NAME {
        let address = std::env::var("GB_TRACE_ADDR")
            .ok()
            .and_then(|v| u16::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
            .unwrap_or(0xC000);
        let p = terminalgb_plugin_example_tracer::Tracer::new(address);
        return Some(match gb.add_plugin(Box::new(p)) {
            Ok(()) => format!("plugin example-tracer attached, watching ${address:04X}"),
            Err(e) => format!("plugin example-tracer: {e}"),
        });
    }

    // Silence "unused" warnings in a build with no plugins at all.
    let _ = (gb, cfg, rom, name);
    None
}

/// Build the agent plugin from configuration.
///
/// **Nothing is defaulted into existence here.** With no `agent_command` there
/// is no driver, and rather than attaching an agent that does nothing this
/// says so — an inert plugin in the status bar is worse than an honest error.
#[cfg(feature = "plugin-agent")]
fn build_agent(
    cfg: &crate::screen::config::Config,
) -> Result<terminalgb_plugin_agent::AgentPlugin, String> {
    use terminalgb_plugin_agent::{
        governor::Budget, subprocess::CommandDriver, Config as AgentCfg,
    };

    let command = std::env::var("GB_AGENT_CMD")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.agent_command.clone())
        .ok_or_else(|| {
            "no driver: set agent_command (or GB_AGENT_CMD) to a program that \
             reads the state on stdin and prints a plan — see docs/claude-plays.md"
                .to_string()
        })?;
    let mut parts = command.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| "agent_command is empty".to_string())?;
    let args: Vec<String> = parts.map(str::to_string).collect();

    let objective = std::env::var("GB_AGENT_OBJECTIVE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.agent_objective.clone())
        .unwrap_or_default();

    let log = std::env::var("GB_AGENT_LOG")
        .ok()
        .map(std::path::PathBuf::from);

    Ok(terminalgb_plugin_agent::AgentPlugin::new(
        Box::new(CommandDriver::new(program, args)),
        AgentCfg {
            budget: Budget {
                max_calls: cfg.agent_max_calls,
                max_usd: cfg.agent_max_usd,
                ..Budget::default()
            },
            objective,
            log,
            ..AgentCfg::default()
        },
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_uses_whole_area_and_centres() {
        // 100x40 cells, square 8x16px font. Image 160x144.
        let (rect, scale) = fit_metrics(Rect::new(0, 0, 100, 40), 160, 144, 8, 16);
        // Must stay within the area.
        assert!(rect.width <= 100 && rect.height <= 40);
        // Must be centred: equal margins on each axis (±1 for rounding).
        let left = rect.x;
        let right = 100 - (rect.x + rect.width);
        assert!((left as i32 - right as i32).abs() <= 1, "not h-centred");
        let top = rect.y;
        let bottom = 40 - (rect.y + rect.height);
        assert!((top as i32 - bottom as i32).abs() <= 1, "not v-centred");
        assert!(scale > 1.0, "should upscale into a large area");
    }

    #[test]
    fn fit_scales_up_with_bigger_terminal() {
        let small = fit_metrics(Rect::new(0, 0, 40, 20), 160, 144, 8, 16).1;
        let big = fit_metrics(Rect::new(0, 0, 200, 80), 160, 144, 8, 16).1;
        // No artificial cap: a bigger window yields a bigger picture.
        assert!(big > small, "bigger terminal must give bigger scale");
    }

    #[test]
    fn fit_no_cap_can_exceed_four() {
        // The old code capped scale at 4. A large window must beat that.
        let (_r, scale) = fit_metrics(Rect::new(0, 0, 400, 200), 160, 144, 10, 20);
        assert!(scale > 4.0, "scale {scale} should be uncapped");
    }

    #[test]
    fn fit_gives_many_smooth_tiers_not_three() {
        // The captain's complaint was "three tiers". Sweep terminal height with
        // ample width (the normal, height-limited case) and count distinct
        // effective scales. Height-limited fit yields ~one tier per row.
        let mut scales = std::collections::BTreeSet::new();
        let mut prev = 0.0f32;
        for rows in 18..=70u16 {
            let (_r, s) = fit_metrics(Rect::new(0, 0, 200, rows), 160, 144, 8, 16);
            // Monotonic non-decreasing as the window grows.
            assert!(s + 1e-3 >= prev, "scale must not shrink as window grows");
            prev = s;
            scales.insert((s * 100.0) as u32);
        }
        assert!(
            scales.len() >= 20,
            "expected many smooth tiers, got {}",
            scales.len()
        );
    }

    #[test]
    fn fit_preserves_aspect_across_sizes() {
        // The on-screen picture must keep the DMG's 10:9 (~1.111) aspect at
        // every size, accounting for the non-square cell (8x16 here).
        for rows in [18u16, 24, 30, 45, 60, 78] {
            let (r, _s) = fit_metrics(Rect::new(0, 0, 200, rows), 160, 144, 8, 16);
            let px_w = r.width as f32 * 8.0;
            let px_h = r.height as f32 * 16.0;
            let aspect = px_w / px_h;
            assert!(
                (aspect - 160.0 / 144.0).abs() < 0.05,
                "aspect {aspect} drifted from 1.111 at {rows} rows"
            );
        }
    }

    #[test]
    fn fit_handles_degenerate_area() {
        let (rect, scale) = fit_metrics(Rect::new(0, 0, 0, 0), 160, 144, 8, 16);
        assert_eq!(scale, 1.0);
        assert_eq!(rect, Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn fit_tiny_area_stays_in_bounds() {
        let (rect, _s) = fit_metrics(Rect::new(5, 5, 3, 2), 160, 144, 8, 16);
        assert!(rect.width >= 1 && rect.height >= 1);
        assert!(rect.x + rect.width <= 5 + 3);
        assert!(rect.y + rect.height <= 5 + 2);
    }

    #[test]
    fn popup_is_centred_and_bounded() {
        let a = Rect::new(0, 0, 80, 40);
        let p = popup_area(a, 50, 50);
        assert_eq!(p.width, 40);
        assert_eq!(p.height, 20);
        assert_eq!(p.x, 20);
        assert_eq!(p.y, 10);
    }

    #[test]
    fn popup_clamped_to_small_screen() {
        let a = Rect::new(0, 0, 8, 4);
        let p = popup_area(a, 90, 90);
        assert!(p.width <= 8 && p.height <= 4);
    }

    #[test]
    fn recent_ordering_puts_recents_first_and_flags() {
        use romlib::entry_from_bytes;
        let mut rom = vec![0u8; 0x150];
        rom[0x147] = 0x00;
        let a = entry_from_bytes(Path::new("/r/a.gb"), &rom);
        let b = entry_from_bytes(Path::new("/r/b.gb"), &rom);
        let c = entry_from_bytes(Path::new("/r/c.gb"), &rom);
        // Recent = [c, a]; b is not recent.
        let recents = vec![PathBuf::from("/r/c.gb"), PathBuf::from("/r/a.gb")];
        let (ordered, flags) =
            order_with_recent(vec![a.clone(), b.clone(), c.clone()], &recents);
        assert_eq!(ordered[0].path, c.path);
        assert_eq!(ordered[1].path, a.path);
        assert_eq!(ordered[2].path, b.path);
        assert_eq!(flags, vec![true, true, false]);
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("SHORT", 20), "SHORT");
        assert_eq!(truncate("A VERY LONG GAME TITLE", 6), "A VER…");
    }

    #[test]
    fn protocol_string_roundtrip() {
        assert_eq!(protocol_from_str("kitty"), Some(ProtocolType::Kitty));
        assert_eq!(protocol_from_str("ITERM"), Some(ProtocolType::Iterm2));
        assert_eq!(protocol_from_str("nonsense"), None);
        assert_eq!(protocol_name(ProtocolType::Sixel), "sixel");
    }

    /// The config validates `protocol` without being able to see the frontend's
    /// parser (it is not behind the `tui` feature), so the two lists can drift.
    /// If they do, a value the config accepts is silently ignored by the
    /// frontend — the exact "forcing a protocol does nothing" bug the
    /// validation was added to stop.
    #[test]
    fn every_configurable_protocol_is_understood_by_the_frontend() {
        for name in crate::screen::config::PROTOCOLS {
            assert!(
                protocol_from_str(name).is_some(),
                "config accepts protocol = {name}, but the frontend ignores it"
            );
        }
    }

    /// The ladder, at the level that decides what the user sees. Half-blocks is
    /// the *last* resort: a terminal that advertises Sixel must not get it.
    #[test]
    fn the_picker_prefers_sixel_over_halfblocks_and_kitty_over_sixel() {
        use termcaps::{Graphics, TermCaps};
        let sixel_only = TermCaps {
            answered: true,
            kitty: false,
            sixel: true,
            cell: Some((9, 20)),
        };
        assert_eq!(sixel_only.best(), Some(Graphics::Sixel));

        let both = TermCaps {
            kitty: true,
            ..sixel_only
        };
        assert_eq!(both.best(), Some(Graphics::Kitty));

        // Answered, advertises neither: the caller keeps the env guess or
        // half-blocks, and `best()` says so by returning nothing.
        let neither = TermCaps {
            sixel: false,
            ..sixel_only
        };
        assert_eq!(neither.best(), None);
    }

    /// The capability word is packed into one atomic so `build_picker` can read
    /// it without a lock; a packing bug would silently downgrade every session.
    #[test]
    fn probed_capabilities_survive_the_round_trip_through_the_atomic() {
        for caps in [
            termcaps::TermCaps {
                answered: true,
                kitty: true,
                sixel: false,
                cell: Some((7, 14)),
            },
            termcaps::TermCaps {
                answered: true,
                kitty: false,
                sixel: true,
                cell: Some((9, 20)),
            },
            termcaps::TermCaps {
                answered: true,
                kitty: false,
                sixel: false,
                cell: None,
            },
            termcaps::TermCaps::default(),
        ] {
            GFX_CAPS.store(pack_caps(caps), Ordering::Relaxed);
            assert_eq!(probed_caps(), caps);
        }
        GFX_CAPS.store(0, Ordering::Relaxed);
    }

    /// Sixel and half-blocks have no cell-addressed placement, so they have no
    /// `TermImage` — but that is not the `render_path = legacy` escape hatch and
    /// must not be reported as it. `full` says the whole picture goes over the
    /// wire every frame, which is the cost worth seeing.
    #[test]
    fn the_status_row_does_not_call_sixel_legacy() {
        let app = test_app(ProtocolType::Sixel, RenderPath::Fast).unwrap();
        let s = app.session.as_ref().expect("test_app starts a session");
        assert!(s.term_image.is_none(), "sixel has no cell placement");
        assert_eq!(s.transport_label(ProtocolType::Sixel), "full");
        assert_eq!(s.transport_label(ProtocolType::Halfblocks), "full");
        // The escape hatch keeps its own name.
        assert_eq!(s.transport_label(ProtocolType::Kitty), "legacy");
        // ...and the protocol itself is named, so `sixel full` is what shows.
        assert_eq!(protocol_name(app.picker.protocol_type()), "sixel");
    }

    #[test]
    fn key_index_roundtrips() {
        for i in 0..8usize {
            assert_eq!(key_index(key_from_index(i)), i);
        }
    }

    #[test]
    fn speed_ladder_intervals_and_default() {
        assert_eq!(SPEED_LADDER[SPEED_NORMAL].label, "1x");
        assert_eq!(SPEED_LADDER[SPEED_NORMAL].interval(), FRAME_TIME);
        // 2x halves the frame interval; 0.5x doubles it.
        let two = SPEED_LADDER.iter().find(|l| l.label == "2x").unwrap();
        assert_eq!(two.interval(), FRAME_TIME / 2);
        let half = SPEED_LADDER.iter().find(|l| l.label == "0.5x").unwrap();
        assert_eq!(half.interval(), FRAME_TIME * 2);
        assert!(SPEED_LADDER.last().unwrap().uncapped);
    }

    #[test]
    fn state_path_sanitises_and_namespaces() {
        let p = state_path("Zelda / DX", 2);
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(!name.contains('/') && !name.contains(' '));
        assert!(name.ends_with(".slot2.gbss"));
        assert_ne!(state_path("Zelda", 1), state_path("Zelda", 2));
        assert_ne!(state_path("Zelda", 1), state_path("Mario", 1));
        assert!(state_path("", 1)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rom"));
    }

    #[test]
    fn display_pipeline_upscales_with_every_algorithm() {
        // Exercise the real TUI display stages headlessly: emulate the vendored
        // dmg-acid2 ROM a few frames, then run palette-map + upscale through
        // each algorithm and check the produced dimensions.
        use crate::upscale::Algorithm;
        let rom = std::fs::read("third_party/dmg-acid2/dmg-acid2.gb")
            .expect("vendored acid2 ROM present");
        let mut gb = Gameboy::new(rom, None);
        for _ in 0..5 {
            gb.frame();
        }
        let mut prep = FramePrep::new();
        for (algo, factor, want) in [
            (Algorithm::Nearest, 1, (160usize, 144usize)),
            (Algorithm::Scale2x, 3, (480, 432)),
            (Algorithm::Mmpx, 2, (320, 288)),
            (Algorithm::LcdGrid, 4, (640, 576)),
        ] {
            let mut up = Upscaler::new(algo, factor);
            let mapped = prep.map(gb.image(), Palette::Green).to_vec();
            assert_eq!(up.output_size(160, 144), want, "{algo:?}");
            let out = up.apply(&mapped, 160, 144);
            assert_eq!(out.len(), want.0 * want.1 * 4, "{algo:?}");
        }
    }

    // -- Pacing: emulation must not be coupled to the display -------------

    #[test]
    fn frames_due_is_zero_before_the_deadline() {
        let t0 = Instant::now();
        let (n, next) = frames_due(t0, t0 + FRAME_TIME, FRAME_TIME);
        assert_eq!(n, 0);
        assert_eq!(next, t0 + FRAME_TIME);
    }

    #[test]
    fn frames_due_advances_by_exact_periods_not_to_now() {
        // 2.5 frame periods late => 3 frames due, deadline advanced by exactly
        // 3 periods (never reset to `now`, which would leak drift).
        let t0 = Instant::now();
        let deadline = t0;
        let now = t0 + FRAME_TIME * 5 / 2;
        let (n, next) = frames_due(now, deadline, FRAME_TIME);
        assert_eq!(n, 3);
        assert_eq!(next, deadline + FRAME_TIME * 3);
    }

    #[test]
    fn frames_due_resyncs_after_a_long_stall_instead_of_fast_forwarding() {
        let t0 = Instant::now();
        let now = t0 + FRAME_TIME * 500; // suspended laptop, say
        let (n, next) = frames_due(now, t0, FRAME_TIME);
        assert_eq!(n, MAX_CATCHUP, "must not replay 500 frames at once");
        assert_eq!(next, now + FRAME_TIME);
    }

    #[test]
    fn a_slow_display_drops_pictures_not_emulated_frames() {
        // Thirty simulated seconds of the real loop arithmetic with a draw that
        // costs 50 ms — three times the frame budget, i.e. the captain's
        // window. Emulation must still run at 59.7275 fps; the display gets
        // ~20, and the difference is dropped *pictures*, not emulated frames.
        let start = Instant::now();
        let draw_cost = Duration::from_millis(50);
        let horizon = Duration::from_secs(30);
        let mut now = start;
        let mut next_frame = start;
        let mut next_draw = start;
        let (mut emulated, mut drawn) = (0u32, 0u32);
        while now < start + horizon {
            let (due, deadline) = frames_due(now, next_frame, FRAME_TIME);
            emulated += due;
            next_frame = deadline;
            if now >= next_draw {
                drawn += 1;
                let started = now;
                now += draw_cost; // the draw blocks the loop for this long
                next_draw = next_draw_at(started, draw_cost);
            } else {
                // Otherwise the loop waits for whichever deadline is nearer.
                now = next_frame
                    .min(next_draw)
                    .max(now + Duration::from_micros(100));
            }
        }
        let secs = now.duration_since(start).as_secs_f32();
        let emu_rate = emulated as f32 / secs;
        let draw_rate = drawn as f32 / secs;
        // The accumulator must never be more than one batch behind the clock:
        // that is the invariant that keeps the game at true speed.
        assert!(
            now.saturating_duration_since(next_frame) <= FRAME_TIME * MAX_CATCHUP,
            "emulation fell irrecoverably behind"
        );
        assert!(
            (emu_rate - DMG_HZ).abs() < 0.15,
            "emulation ran at {emu_rate:.1} fps, not {DMG_HZ} — the game would lag"
        );
        assert!(
            (18.0..=21.0).contains(&draw_rate),
            "expected ~20 displayed fps with a 50 ms draw, got {draw_rate:.1}"
        );
    }

    /// The same invariant for the half we could not previously see.
    ///
    /// A *slow draw* was always handled — `next_draw_at` spaced draws by their
    /// measured cost. But with the shared-memory transport the draw costs 0.2 ms
    /// however big the window is, and all the expense moved to the terminal,
    /// where nothing measured it. So the loop happily issued 59.7 pictures a
    /// second to a terminal that could take 20, the surplus was coalesced away
    /// inside it, and `fps` read 59 throughout. Feeding the measured terminal
    /// cost into the same pacing arithmetic is what makes that number true.
    #[test]
    fn a_slow_terminal_drops_pictures_not_emulated_frames() {
        let start = Instant::now();
        // A terminal that needs 50 ms per frame — three times the budget — with
        // a draw of our own that costs almost nothing, which is the real shape.
        let mut termcost = termcost::TerminalCost::new(true, Duration::ZERO);
        let our_cost = Duration::from_micros(200);
        let terminal = Duration::from_millis(50);
        let horizon = Duration::from_secs(30);
        let mut now = start;
        let mut next_frame = start;
        let mut next_draw = start;
        let (mut emulated, mut drawn) = (0u32, 0u32);
        let mut fences = 0u32;
        while now < start + horizon {
            let (due, deadline) = frames_due(now, next_frame, FRAME_TIME);
            emulated += due;
            next_frame = deadline;
            if now >= next_draw {
                drawn += 1;
                let started = now;
                now += our_cost;
                let sampled = now;
                termcost.observe(sampled, (133, 60), || {
                    fences += 1;
                    now += terminal; // the fence blocks for the round trip
                    Some(terminal)
                });
                let interval = termcost.draw_interval(FRAME_TIME);
                next_draw = next_draw_at(started, our_cost.max(interval));
            } else {
                now = next_frame
                    .min(next_draw)
                    .max(now + Duration::from_micros(100));
            }
        }
        let secs = now.duration_since(start).as_secs_f32();
        let emu_rate = emulated as f32 / secs;
        let draw_rate = drawn as f32 / secs;
        assert!(
            (emu_rate - DMG_HZ).abs() < 0.15,
            "emulation ran at {emu_rate:.1} fps, not {DMG_HZ} — the game would lag"
        );
        assert!(
            (18.0..=21.0).contains(&draw_rate),
            "expected ~20 issued fps against a 50 ms terminal, got {draw_rate:.1}"
        );
        // And the measurement itself must stay cheap: three fences per
        // RESAMPLE_AFTER, not one per frame.
        let expected_fences = (horizon.as_secs_f32()
            / termcost::RESAMPLE_AFTER.as_secs_f32())
        .ceil() as u32
            * termcost::SAMPLES as u32;
        assert!(
            fences <= expected_fences + termcost::SAMPLES as u32,
            "fenced {fences} times in 30 s — sampling, not per-frame, is the point"
        );
    }

    #[test]
    fn a_fast_display_is_capped_at_the_panel_rate() {
        let t = Instant::now();
        // A draw that costs nothing still may not run more often than 59.7 Hz.
        assert_eq!(next_draw_at(t, Duration::ZERO), t + FRAME_TIME);
        assert_eq!(next_draw_at(t, Duration::from_millis(5)), t + FRAME_TIME);
        assert_eq!(
            next_draw_at(t, Duration::from_millis(40)),
            t + Duration::from_millis(40)
        );
    }

    // -- End-to-end render path ------------------------------------------
    //
    // These drive the *real* `App::render` against a `TestBackend`, so they
    // assert on the bytes the frontend would actually put on the wire. That is
    // as close to "watch it run" as a headless test gets, and it is what makes
    // the performance claim checkable rather than asserted.

    /// An `App` with a fixed protocol and cell size, playing the vendored
    /// acid2 ROM. The ROM is committed, so a missing one is a hard failure
    /// rather than a silently-skipped test.
    /// **"Not built in" is told apart from "unknown".** Two different problems
    /// with two different fixes, and one message for both sends the user
    /// hunting for a typo that is not there.
    ///
    /// `example-tracer` is the case that makes this testable in every build:
    /// it is deliberately outside `standard-plugins`, so in a default build it
    /// is a name we know and a crate we did not link.
    #[test]
    fn a_known_but_unbuilt_plugin_says_how_to_build_it_in() {
        let rom = std::fs::read("third_party/dmg-acid2/dmg-acid2.gb")
            .expect("vendored acid2 ROM present");
        let mut gb = Gameboy::new(rom.clone(), None);
        let cfg = crate::screen::config::Config::default();

        // Known to the registry, so we can name the feature that supplies it.
        assert!(REGISTRY.iter().any(|(n, _)| *n == "example-tracer"));

        #[cfg(not(feature = "plugin-example-tracer"))]
        {
            assert_eq!(attach_named(&mut gb, &cfg, &rom, "example-tracer"), None);
            let notes = {
                let mut c = cfg.clone();
                c.plugins = vec!["example-tracer".to_string()];
                // GB_PLUGINS would win over the config; this test reads config.
                std::env::remove_var("GB_PLUGINS");
                attach_plugins(&mut gb, &c, &rom)
            };
            let note = notes
                .iter()
                .find(|n| n.contains("example-tracer"))
                .expect("the attempt must be reported");
            assert!(note.contains("not built into this binary"), "{note}");
            assert!(note.contains("--features plugin-example-tracer"), "{note}");
        }

        // A name nothing knows is a different message entirely.
        let mut c = cfg.clone();
        c.plugins = vec!["definitely-not-a-plugin".to_string()];
        std::env::remove_var("GB_PLUGINS");
        let notes = attach_plugins(&mut gb, &c, &rom);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("unknown plugin 'definitely-not-a-plugin'")),
            "{notes:?}"
        );
    }

    /// A shelf with one album, without touching a ROM directory or the probing
    /// worker: [`Library::from_roms`] over a hand-assembled cartridge.
    #[cfg(feature = "music")]
    fn shelf_app() -> Option<App> {
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast)?;
        let rom = crate::music::testrom::tone_rom();
        let entry = romlib::entry_from_bytes(Path::new("/x/tone.gb"), &rom);
        let lib = crate::music::Library::from_roms(
            vec![(entry, rom)],
            crate::music::library::ProbeOptions {
                seconds: 1.5,
                cover: crate::music::cover::Options {
                    sample_every: 4,
                    warmup: 4,
                    keep: 4,
                    keep_states: false,
                },
            },
        );
        assert!(
            lib.get(0).map(|a| !a.covers.is_empty()).unwrap_or(false),
            "the fixture must produce a cover for this test to mean anything"
        );
        let mut state = mv::MusicState::new(lib, std::env::temp_dir(), 30.0);
        state.image = app.new_cover_image();
        app.music = Some(state);
        app.screen = Screen::Shelf;
        Some(app)
    }

    /// The help overlay renders **whole** on the shelf.
    ///
    /// Observed live before [`App::render_cover`] learned to take the cover off
    /// the terminal for a popup: the box drew with the right-hand half of every
    /// line missing, cut at exactly the cover's edge.
    ///
    /// **Honest limitation: this test did not catch that, and cannot.** It was
    /// found by screenshotting a real terminal, and it is fixed and verified the
    /// same way (same window size, before and after). Removing the fix leaves
    /// this test green, because `ratatui`'s `Clear` widget calls `Cell::reset`,
    /// which clears `skip` over the popup's own rectangle — so in a
    /// `TestBackend` the popup always flushes. What this test *is* worth is a
    /// regression guard on the overlay's content and box integrity, which is
    /// cheap and which nothing else covers. Do not read it as proof of the
    /// clipping fix; the screenshots are that.
    ///
    /// Asserted on **rendered output**, not on the `skip` flags: `skip` lives
    /// in ratatui's buffer and what a reader sees is the backend's.
    #[cfg(feature = "music")]
    #[test]
    fn the_help_overlay_is_not_eaten_by_the_cover() {
        use ratatui::backend::TestBackend;
        // Lines that must survive whole, and that fit inside the popup even
        // at the smallest size below. Each is long enough to cross the cover's
        // rectangle, which is what the bug truncated.
        const WANT: [&str; 2] = [
            "save what you are hearing as a WAV",
            "play the selected album",
        ];
        for (cols, rows) in [(80u16, 24u16), (120, 40), (150, 45), (200, 60)] {
            let Some(mut app) = shelf_app() else { return };
            let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            // Twice: a cell is only marked `skip` on a frame where the picture
            // is NOT being re-sent, so the first draw cannot show the bug.
            terminal.draw(|f| app.render(f)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();

            app.overlay = Overlay::Help;
            terminal.draw(|f| app.render(f)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let screen: String = (0..rows)
                .map(|y| {
                    (0..cols)
                        .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            for want in WANT {
                assert!(
                    screen.contains(want),
                    "{cols}x{rows}: the help overlay lost {want:?} — the cover \
                     ate it\n{screen}"
                );
            }
            // And the box itself is whole: the bug clipped horizontally, so a
            // top border that reaches its own right corner is the tightest
            // single check there is.
            let top = screen
                .lines()
                .find(|l| l.contains("┌ ? "))
                .unwrap_or_else(|| panic!("{cols}x{rows}: no help box\n{screen}"));
            assert!(
                top.trim_end().ends_with('┐'),
                "{cols}x{rows}: the help box was clipped: {top:?}"
            );
        }
    }

    /// The shelf's layout rule, asserted against **rendered output**: nothing is
    /// written into the rectangle the cover owns.
    #[cfg(feature = "music")]
    #[test]
    fn nothing_is_written_over_the_cover_on_the_shelf() {
        use ratatui::backend::TestBackend;
        for (cols, rows) in [(60u16, 18u16), (100, 30), (150, 45), (213, 55)] {
            let Some(mut app) = shelf_app() else { return };
            let (fw, fh) = app.picker.font_size();
            let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let cover = mv::fit_cover(
                mv::layout(Rect::new(0, 0, cols, rows), fw, fh).cover,
                fw,
                fh,
            );
            if cover.width == 0 {
                continue;
            }
            let buf = terminal.backend().buffer().clone();
            for y in cover.y..cover.bottom() {
                for x in cover.x..cover.right() {
                    let sym = buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" ");
                    assert_eq!(
                        sym, " ",
                        "{cols}x{rows}: {sym:?} written at ({x},{y}), inside the \
                         cover's rectangle {cover:?}"
                    );
                }
            }
        }
    }

    fn test_app(protocol: ProtocolType, path: RenderPath) -> Option<App> {
        test_app_video(protocol, path, video_for_test())
    }

    /// **Music mode must never write a save file.** This is the property the
    /// whole feature rests on: it boots every cartridge on the shelf, and a
    /// music player that quietly overwrote a real Pokémon save with the
    /// pristine RAM of a thirty-second listen would be the worst bug this
    /// repository has shipped (see `src/mbc/battery.rs`, which was written
    /// after exactly that happened).
    ///
    /// It is guaranteed structurally — [`load_music_session`] passes `None` as
    /// the cartridge path, so the MBC has nowhere to write — and this drives
    /// the real function to prove it, on a cartridge whose header says
    /// MBC3+RAM+BATTERY.
    #[cfg(feature = "music")]
    #[test]
    fn playing_an_album_never_writes_a_save_beside_the_rom() {
        let dir =
            std::env::temp_dir().join(format!("gb-music-tui-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // A cartridge that *writes* to its battery-backed RAM — see
        // `testrom::battery_rom`. One that merely declares RAM proves nothing.
        let rom = crate::music::testrom::battery_rom();
        let path = dir.join("battery.gb");
        std::fs::write(&path, &rom).unwrap();

        let cfg = Config::default();
        let (mut gb, entry, _audio) =
            load_music_session(&path, &cfg).expect("the fixture is a valid cartridge");
        assert_eq!(entry.title, "GB BATTERY");
        // Long enough for the autosave's quiet-frame timer to fire twice over.
        for _ in 0..200 {
            gb.frame();
        }
        gb.flush_battery();
        drop(gb);

        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|f| f.ok())
            .map(|f| f.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "battery.gb")
            .collect();
        assert!(
            leftovers.is_empty(),
            "music mode must not write save files: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second console must actually run, be swappable, and be **visible** —
    /// the same standing rule as an attached plugin. A link that is quietly
    /// doing nothing is the failure mode this row exists to rule out.
    #[test]
    fn a_linked_second_console_runs_and_is_visible_and_swappable() {
        let Some(mut app) = test_app_linked() else {
            return;
        };
        assert!(
            app.session.as_ref().unwrap().link.is_some(),
            "`link = <rom>` should have booted a second console"
        );

        // Both machines must advance. The partner is a separate `Gameboy`, so
        // its program counter moving is the honest evidence that the cable's
        // loop is stepping it and not just the focused one.
        let before = app
            .session
            .as_ref()
            .unwrap()
            .link
            .as_ref()
            .unwrap()
            .gb
            .debug_pc();
        let mut moved = false;
        for _ in 0..8 {
            app.step_frame();
            let now = app
                .session
                .as_ref()
                .unwrap()
                .link
                .as_ref()
                .unwrap()
                .gb
                .debug_pc();
            moved |= now != before;
        }
        assert!(moved, "the linked console never executed an instruction");

        // Visible: the Status panel names it.
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(150, 45)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("Link"),
            "the Status panel must show the link — a second console cannot be \
             active and invisible.\n{text}"
        );

        // Swappable, and the swap really moves the machines.
        let focused = app.session.as_ref().unwrap().gb.debug_pc();
        let other = app
            .session
            .as_ref()
            .unwrap()
            .link
            .as_ref()
            .unwrap()
            .gb
            .debug_pc();
        app.handle_game_key(KeyEvent::from(KeyCode::Char('W')));
        assert_eq!(app.session.as_ref().unwrap().gb.debug_pc(), other);
        assert_eq!(
            app.session
                .as_ref()
                .unwrap()
                .link
                .as_ref()
                .unwrap()
                .gb
                .debug_pc(),
            focused
        );
    }

    /// The auto-discovered cable must be **visible** and must reach the
    /// trainer panel, not only the Status row. No sockets and no environment
    /// variables here: a switched-off cable exercises the same wiring, and the
    /// transport itself is proved in `crate::link::peer`.
    #[test]
    fn a_discovered_cable_is_visible_in_both_panels() {
        let Some(mut app) = test_app(ProtocolType::Halfblocks, RenderPath::Fast) else {
            return;
        };
        app.session.as_mut().unwrap().peer =
            Some(Box::new(crate::link::peer::LinkPeer::off()));

        let (short, long) = app.link_summary().expect("a cable should be described");
        assert_eq!(short, "off");
        assert!(long.contains("unplugged"), "{long}");
        assert!(
            short.chars().count() <= 18,
            "the Status row is 18 columns: {short:?}"
        );

        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(150, 45)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("Link"),
            "a cable that nobody can see is a cable nobody can debug\n{text}"
        );
    }

    /// The link key does the right thing for whichever kind of cable is
    /// attached, and says so rather than doing nothing.
    #[test]
    fn the_link_key_toggles_a_discovered_cable() {
        let Some(mut app) = test_app(ProtocolType::Halfblocks, RenderPath::Fast) else {
            return;
        };
        app.session.as_mut().unwrap().peer =
            Some(Box::new(crate::link::peer::LinkPeer::off()));
        assert_eq!(app.link_summary().unwrap().0, "off");

        // Plugging in runs discovery, which needs a rendezvous directory it
        // cannot share with another test — point it somewhere private.
        let dir = std::env::temp_dir()
            .join(format!("terminalgb-linkkey/{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        env::set_var("GB_LINK_DIR", &dir);
        app.handle_game_key(KeyEvent::from(KeyCode::Char('W')));
        assert_ne!(
            app.link_summary().unwrap().0,
            "off",
            "W should have plugged the cable back in"
        );
        app.handle_game_key(KeyEvent::from(KeyCode::Char('W')));
        assert_eq!(
            app.link_summary().unwrap().0,
            "off",
            "and unplugged it again"
        );
        env::remove_var("GB_LINK_DIR");
    }

    /// An unreadable second cartridge costs you the cable and **nothing else**
    /// — the same rule the frontend follows for every other absent capability.
    #[test]
    fn a_missing_link_rom_is_a_warning_not_a_refusal() {
        let Some(app) = test_app_linked_to("/definitely/not/a/rom.gb") else {
            return;
        };
        assert!(app.session.is_some(), "the game must still have started");
        assert!(app.session.as_ref().unwrap().link.is_none());
        assert!(
            app.config_warnings.iter().any(|w| w.starts_with("link:")),
            "and the player must be told why: {:?}",
            app.config_warnings
        );
    }

    fn test_app_linked() -> Option<App> {
        test_app_linked_to("third_party/dmg-acid2/dmg-acid2.gb")
    }

    fn test_app_linked_to(link: &str) -> Option<App> {
        isolate_state_dir();
        let rom = std::fs::read("third_party/dmg-acid2/dmg-acid2.gb").ok()?;
        let mut picker = Picker::from_fontsize((8, 16));
        picker.set_protocol_type(ProtocolType::Halfblocks);
        let config = Config {
            link: LinkSetting::Rom(link.to_string()),
            ..Config::default()
        };
        let mut app = App::build(
            config,
            Recent::default(),
            Vec::new(),
            picker,
            video_for_test(),
            None,
        );
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("gameboy-link-test/{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let rom_path = dir.join("acid2.gb");
        std::fs::write(&rom_path, &rom).ok()?;
        app.start_game(&rom_path);
        Some(app)
    }

    /// Point `$XDG_STATE_HOME` at a scratch directory, once per test process.
    ///
    /// The frontend persists things there — the recent list, save slots, and
    /// now the playback volume, which is written on every `,` / `.` / `m`
    /// press. A test must never write into the *user's* state directory: a
    /// `cargo test` run would silently reset the volume of whoever ran it.
    fn isolate_state_dir() {
        static ONCE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        let dir = ONCE.get_or_init(|| {
            let d = std::env::temp_dir()
                .join(format!("gameboy-test-state/{}", std::process::id()));
            let _ = std::fs::create_dir_all(&d);
            d
        });
        env::set_var("XDG_STATE_HOME", dir);
    }

    /// Video-out is off by default, in tests as in production.
    fn video_for_test() -> VideoOut {
        VideoOut::disabled()
    }

    /// [`test_app`] with a raw video stream attached.
    fn test_app_video(
        protocol: ProtocolType,
        path: RenderPath,
        video: VideoOut,
    ) -> Option<App> {
        isolate_state_dir();
        let rom = std::fs::read("third_party/dmg-acid2/dmg-acid2.gb")
            .expect("vendored dmg-acid2 ROM present");
        let mut picker = Picker::from_fontsize((8, 16));
        picker.set_protocol_type(protocol);
        let config = Config {
            render_path: path,
            palette: Palette::Green,
            ..Config::default()
        };
        let mut app =
            App::build(config, Recent::default(), Vec::new(), picker, video, None);
        // A private copy per call: tests run in parallel, the MBC writes a
        // `.gbsave` next to the ROM, and a shared path means one test can be
        // reading a file another is truncating.
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("gameboy-render-test/{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let rom_path = dir.join("acid2.gb");
        std::fs::write(&rom_path, &rom).expect("write temp ROM");
        app.start_game(&rom_path);
        assert!(
            app.session.is_some(),
            "ROM failed to load: {:?}",
            app.message
        );
        Some(app)
    }

    /// Draw one frame at the given terminal size and return the number of image
    /// bytes the frontend would write to the terminal for it.
    fn drawn_bytes(app: &mut App, cols: u16, rows: u16) -> usize {
        drawn_wire(app, cols, rows).len()
    }

    /// Draw one frame and return the image sequence the loop would flush.
    fn drawn_wire(app: &mut App, cols: u16, rows: u16) -> String {
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let wire = app
            .session
            .as_ref()
            .map(|s| s.pending_wire.clone())
            .unwrap_or_default();
        if let Some(s) = app.session.as_mut() {
            s.pending_wire.clear(); // the real loop flushes and clears it
        }
        wire
    }

    #[test]
    fn transmitted_bytes_do_not_grow_with_the_window() {
        // The captain's bug, as a test: at 320x90 cells the old path encoded a
        // 1568x1408 image every frame (11.6 MB); the new one sends the same
        // 160x144 image whatever the window size. (Direct placement, the
        // default, does not even pay kitty's per-cell placeholder grid.)
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        let small = drawn_bytes(&mut app, 80, 24);
        app.session.as_mut().unwrap().gb.frame();
        let large = drawn_bytes(&mut app, 320, 90);
        assert!(small > 200, "expected a real payload, got {small} bytes");
        assert!(
            large < 300_000,
            "a 1568x1408 display cost {large} bytes/frame (legacy: 11.6 MB)"
        );
        // Since compression landed the payload is small enough that the window
        // must barely move it at all: the old path grew by four orders of
        // magnitude between these two sizes.
        assert!(
            large < small * 4,
            "{small} -> {large} bytes: payload is still following the window"
        );
    }

    #[test]
    fn kitty_placement_asks_the_terminal_to_scale() {
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        let wire = drawn_wire(&mut app, 160, 50);
        assert!(wire.contains("a=T"), "no kitty placement emitted");
        assert!(
            wire.contains("s=160,v=144"),
            "not transmitting the source size"
        );
        let (cols, rows) = app.session.as_ref().unwrap().disp_cells;
        assert!(
            wire.contains(&format!("c={cols},r={rows}")),
            "placement does not cover the fitted rectangle {cols}x{rows}"
        );
    }

    #[test]
    fn an_unchanged_frame_transmits_nothing() {
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        drawn_wire(&mut app, 160, 50);
        let before = app.session.as_ref().unwrap().frames_identical;
        // Draw again without advancing the machine: same picture, so nothing
        // may be encoded or written.
        let again = drawn_wire(&mut app, 160, 50);
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.frames_identical, before + 1, "re-encoded a still frame");
        assert!(again.is_empty(), "retransmitted {} bytes", again.len());
    }

    /// **The captain's live bug.** With the stats overlay on, the picture froze
    /// while the game kept running: the overlay was drawn on top of the image,
    /// replacing the very cells that carry it, so no new image data reached the
    /// terminal until the overlay was closed.
    ///
    /// The fix is twofold — the stats panel takes a rectangle of its own, and
    /// the image is written outside `ratatui`'s cell buffer entirely — and this
    /// asserts what was missing: **new frames keep being transmitted while the
    /// overlay is up.**
    #[test]
    fn the_picture_keeps_updating_with_the_stats_overlay_on() {
        for protocol in [ProtocolType::Kitty, ProtocolType::Iterm2] {
            let mut app = test_app(protocol, RenderPath::Fast).unwrap();
            app.stats = true;
            let first = drawn_wire(&mut app, 200, 60);
            assert!(!first.is_empty(), "{protocol:?}: nothing drawn at all");

            // Three successive *changed* pictures must each be transmitted.
            // The framebuffer is poked directly rather than relying on the ROM
            // to animate: the property under test is "a changed picture reaches
            // the terminal while the overlay is up", and the vendored acid2 ROM
            // draws a still image (which correctly transmits nothing).
            for round in 0..3 {
                app.step_frame();
                if let Some(s) = app.session.as_mut() {
                    s.gb.image_mut()[0] = 17 * (round as u8 + 1);
                }
                let wire = drawn_wire(&mut app, 200, 60);
                assert!(
                    !wire.is_empty(),
                    "{protocol:?}: picture frozen with the stats overlay up (round {round})"
                );
                assert!(
                    wire.contains(match protocol {
                        ProtocolType::Iterm2 => "inline=1",
                        _ => "a=T",
                    }),
                    "{protocol:?}: round {round} sent no image data"
                );
            }
        }
    }

    // ---- screenshots ---------------------------------------------------

    #[test]
    fn utc_stamp_is_a_sortable_calendar_time() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        // `date -u -d '2026-08-13 12:34:56' +%s`
        assert_eq!(utc_stamp(1_786_624_496), "20260813-123456");
        // A leap day, and the day after (the civil-from-days edge case).
        assert_eq!(utc_stamp(1_709_164_800), "20240229-000000");
        assert_eq!(utc_stamp(1_709_251_200), "20240301-000000");
    }

    #[test]
    fn safe_titles_never_escape_a_filename() {
        assert_eq!(safe_title("SUPER MARIO"), "SUPER_MARIO");
        assert_eq!(safe_title("../../etc/passwd"), "etc_passwd");
        assert_eq!(safe_title(""), "rom");
        assert_eq!(safe_title("///"), "rom");
    }

    #[test]
    fn repeated_screenshots_never_overwrite() {
        let dir = std::env::temp_dir().join(format!(
            "gameboy-shot-test/{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "GAME-20260813-123456");
        std::fs::write(&a, b"x").unwrap();
        let b = unique_path(&dir, "GAME-20260813-123456");
        assert_ne!(a, b);
        assert!(b.file_name().unwrap().to_string_lossy().ends_with("-2.png"));
        std::fs::write(&b, b"x").unwrap();
        let c = unique_path(&dir, "GAME-20260813-123456");
        assert!(c.file_name().unwrap().to_string_lossy().ends_with("-3.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole feature, end to end: press the key, a real PNG appears, at the
    /// size the *selected* upscaler produces, and the toast names the file.
    #[test]
    fn the_screenshot_key_writes_a_real_png_and_says_where() {
        let dir = std::env::temp_dir()
            .join(format!("gameboy-shot-test/{}-e2e", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.config.screenshot_dir = Some(dir.to_string_lossy().into_owned());
        app.upscaler
            .set_algorithm(crate::upscale::Algorithm::Scale2x);
        app.upscale_factor = 2;
        for _ in 0..10 {
            app.step_frame();
        }

        app.screenshot(false);
        let toast = app
            .toast
            .as_ref()
            .map(|(t, _)| t.clone())
            .unwrap_or_default();
        assert!(toast.starts_with("Saved "), "{toast}");
        assert!(
            toast.contains("320x288"),
            "displayed shot is upscaled: {toast}"
        );
        let shots: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        assert_eq!(shots.len(), 1, "{shots:?}");
        assert!(toast.contains(&shots[0].to_string_lossy().into_owned()));
        let png = std::fs::read(&shots[0]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "must be a real PNG");
        // IHDR carries the dimensions: bytes 16..24 are width, height.
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 320);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 288);

        // `C` captures the raw framebuffer instead.
        app.screenshot(true);
        let toast = app
            .toast
            .as_ref()
            .map(|(t, _)| t.clone())
            .unwrap_or_default();
        assert!(toast.contains("160x144 raw"), "{toast}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- raw video-out -------------------------------------------------

    /// Video-out must cost nothing — and produce nothing — unless configured.
    #[test]
    fn video_out_is_off_by_default() {
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        assert!(!app.video.is_enabled());
        for _ in 0..10 {
            app.step_frame();
        }
        assert_eq!(app.video.stats().frames, 0);
    }

    /// **The regression test for the whole task.** `video_out` pointing at a
    /// FIFO with no consumer used to park the process in `open(2)` inside
    /// [`open_video`] — before the terminal was even touched, so there was no
    /// UI, no message after the first line, and nothing to do but `kill`.
    /// A media sink must never be able to stop the emulator from starting.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_configured_fifo_with_no_consumer_does_not_stop_the_emulator_starting() {
        use std::ffi::CString;
        let dir = std::env::temp_dir()
            .join(format!("gameboy-vid-test/{}-startup", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fifo = dir.join("gb.raw");
        let _ = std::fs::remove_file(&fifo);
        let c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
            return; // cannot make a FIFO here; nothing to prove
        }

        let mut config = Config::default();
        config.video.out = Some(fifo.to_string_lossy().into_owned());
        config.video.format = Some("yuyv".to_string());
        config.video.size = Some("640x480".to_string());

        let t = Instant::now();
        let (video, template, autostart, warnings) = open_video(&config, Vec::new());
        let took = t.elapsed();
        assert!(
            took < Duration::from_secs(2),
            "startup waited {took:?} on an unread FIFO"
        );
        assert!(!autostart, "nobody asked for it to be live");
        assert_eq!(video.state(), video::StreamState::Off);
        assert!(template.is_some(), "`v` still knows where to send it");
        assert!(
            warnings.iter().any(|w| w.contains("press `v`")),
            "the user must be told how to turn it on: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Defect 1, as a test.** A configured destination is *where* `v` will
    /// send frames. It is not a decision to send them, and the emulator must
    /// come up with the stream **off** — the captain's status row read
    /// `◌ wait gameboy-cam.…` at launch, before he had pressed anything.
    ///
    /// Asserted for both destination kinds, because they take different paths
    /// through [`VideoOut::open`] and only one of them was ever exercised.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_configured_destination_does_not_mean_streaming() {
        use std::ffi::CString;
        let dir = std::env::temp_dir()
            .join(format!("gameboy-vid-test/{}-notlive", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fifo = dir.join("gameboy-cam.raw");
        let _ = std::fs::remove_file(&fifo);
        let c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        let mut dests = vec![dir.join("plain.raw")];
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 {
            dests.push(fifo);
        }
        for dest in dests {
            let mut config = Config::default();
            config.video.out = Some(dest.to_string_lossy().into_owned());
            let (video, template, autostart, _w) = open_video(&config, Vec::new());
            assert!(!autostart, "{}", dest.display());
            assert!(!video.is_enabled(), "{}", dest.display());
            assert_eq!(video.state(), video::StreamState::Off, "{}", dest.display());
            assert!(template.is_some(), "{}", dest.display());
            // Nothing may be created on disk either: opening a plain file
            // truncates it, and a launch that quietly truncates the last
            // recording is the same bug wearing a different hat.
            assert!(
                !dest.exists() || video::is_fifo(&dest),
                "{} was opened at launch",
                dest.display()
            );

            // …and the opt-in still works.
            config.video.autostart = Some("true".to_string());
            let (_v, _t, autostart, _w) = open_video(&config, Vec::new());
            assert!(autostart, "video_autostart = true must be honoured");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stream is driven by **emulated** frames, not displayed ones. This
    /// is the whole reason it is fed from `step_frame`: the terminal drops
    /// display frames on a large window (deliberately — see `crate::termimage`)
    /// and a video capture must not lose those frames with it.
    #[test]
    fn the_video_stream_is_fed_by_emulated_frames_not_displayed_ones() {
        let spec = video::VideoSpec {
            pace: video::Pace::Emulated,
            ..video::VideoSpec::new(video::Destination::Stdout)
        };
        let Some(mut app) = test_app_video(
            ProtocolType::Kitty,
            RenderPath::Fast,
            VideoOut::in_memory(spec),
        ) else {
            return;
        };
        // 30 emulated frames, not one draw.
        for _ in 0..30 {
            app.step_frame();
        }
        assert_eq!(app.video.stats().frames, 30);
        assert_eq!(
            app.video.captured().len(),
            30 * 160 * 144 * 4,
            "native source is the raw framebuffer, RGBA"
        );
    }

    /// The `V` key must start and stop a recording mid-game, name the file
    /// after the cartridge, and leave no doubt about **which of the two jobs**
    /// it is doing: a recording must never present itself as a live feed.
    #[test]
    fn the_record_key_toggles_the_stream_and_names_it_after_the_game() {
        let dir = std::env::temp_dir()
            .join(format!("gameboy-vid-test/{}-toggle", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.config.video_dir = Some(dir.to_string_lossy().into_owned());
        assert_eq!(app.video_status(), "off");

        // On. `V`, because recording is `V`'s job — `v` is the live feed and
        // refuses to record one on its behalf.
        app.toggle_recording();
        assert!(app.video.is_enabled(), "toast: {:?}", app.toast);
        let toast = app.toast.as_ref().map(|(t, _)| t.clone()).unwrap();
        assert!(toast.starts_with("● REC → "), "{toast}");
        assert!(
            app.video_status().starts_with("● REC"),
            "{}",
            app.video_status()
        );
        assert!(
            !app.video_status().contains("LIVE"),
            "a file on disk is not a live feed: {}",
            app.video_status()
        );
        // …and the row says *where*, which is the other half of the confusion.
        assert!(
            app.video_status().contains("DMG_ACID2"),
            "{}",
            app.video_status()
        );

        for _ in 0..5 {
            app.step_frame();
        }

        // Off. A recording is *closed* — that is what makes it a finished
        // file — so nothing is held and the row is a plain `off`.
        app.toggle_recording();
        assert!(!app.video.is_enabled());
        assert!(!app.video.is_idle(), "a recording must not be held open");
        assert_eq!(app.video_status(), "off");
        let toast = app.toast.as_ref().map(|(t, _)| t.clone()).unwrap();
        assert!(toast.contains("stopped"), "{toast}");

        // The file is named after the cartridge and carries its own ffmpeg
        // arguments; the acid2 ROM's header title is "DMG-ACID2".
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].starts_with("DMG_ACID2-"), "{files:?}");
        assert!(files[0].ends_with("-160x144-rgba.raw"), "{files:?}");
        let bytes = std::fs::metadata(dir.join(&files[0])).unwrap().len();
        assert!(bytes > 0 && bytes % (160 * 144 * 4) == 0, "{bytes} bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Defect 2, as a test.** `v` off then `v` on again has to stream, and
    /// keep doing so — ten cycles, no drift in the byte counter, no file
    /// descriptors left behind.
    ///
    /// Run against a FIFO and a plain file (a V4L2 device is exercised by
    /// `examples/video_toggle_probe.rs`, since no CI runner has a
    /// v4l2loopback node).
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn the_stream_survives_ten_on_off_cycles() {
        use std::ffi::CString;
        let dir = std::env::temp_dir()
            .join(format!("gameboy-vid-test/{}-cycles", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let fifo = dir.join("gameboy-cam.raw");
        let c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        let mut dests = vec![dir.join("plain.raw")];
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 {
            dests.push(fifo);
        }
        for dest in dests {
            let what = dest.display().to_string();
            let mut config = Config::default();
            config.video.out = Some(what.clone());
            config.video.pace = Some("emulated".to_string()); // one frame in, one out
            let (video, template, _autostart, _w) = open_video(&config, Vec::new());

            let rom = std::fs::read("third_party/dmg-acid2/dmg-acid2.gb").unwrap();
            let mut picker = Picker::from_fontsize((8, 16));
            picker.set_protocol_type(ProtocolType::Kitty);
            let mut app = App::build(
                config,
                Recent::default(),
                Vec::new(),
                picker,
                video,
                template,
            );
            let rom_path = dir.join("acid2.gb");
            std::fs::write(&rom_path, &rom).unwrap();
            app.start_game(&rom_path);
            assert!(
                app.session.is_some(),
                "ROM failed to load: {:?}",
                app.message
            );
            assert_eq!(app.video_status(), "off", "{what}: launch state");

            // Count only the descriptors pointing into *this* test's directory.
            // `/proc/self/fd` is process-global and the rest of the suite runs
            // in parallel, so a raw count is a race: it failed at random on
            // roughly one run in six with nothing about the video path changed,
            // and more often as the suite grew. Filtering by target makes the
            // measurement mean what the assertion says it means.
            let fds = || {
                std::fs::read_dir("/proc/self/fd").ok().map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter(|e| {
                            std::fs::read_link(e.path())
                                .map(|t| t.starts_with(&dir))
                                .unwrap_or(false)
                        })
                        .count()
                })
            };
            let before = fds();
            for cycle in 0..10 {
                app.toggle_video();
                assert!(
                    app.video.is_enabled(),
                    "{what}: cycle {cycle} would not turn back on (toast {:?})",
                    app.toast
                );
                assert_ne!(
                    app.video_status(),
                    "off",
                    "{what}: cycle {cycle} says off while streaming"
                );
                for _ in 0..5 {
                    app.step_frame();
                }
                let sent = app.video.stats().frames;
                app.toggle_video();
                assert!(
                    !app.video.is_enabled(),
                    "{what}: cycle {cycle} would not stop"
                );
                // A FIFO is a live destination, so it is held and idles; a plain
                // file is a recording, so it is closed. Both are "not sending
                // the game", and the row says which.
                let row = app.video_status();
                assert!(
                    row == "off" || row.starts_with("◍ idle"),
                    "{what}: cycle {cycle} status {row:?}"
                );
                // Game frames offered while off must go nowhere. Only checkable
                // where the emitter survived the stop — a closed recording takes
                // its counters with it, which is right.
                let held = app.video.is_idle();
                for _ in 0..5 {
                    app.step_frame();
                }
                if held {
                    assert_eq!(
                        app.video.stats().frames,
                        sent,
                        "{what}: cycle {cycle} kept streaming the game after stop"
                    );
                }
            }
            if let (Some(a), Some(b)) = (before, fds()) {
                assert!(
                    b <= a + 1,
                    "{what}: leaked file descriptors across ten cycles ({a} -> {b})"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The byte counter must not drift: every cycle streams exactly the frames
    /// it was given and nothing while it is off. Checked on an in-memory sink
    /// so the count is the bytes themselves, not a stat about them.
    #[test]
    fn stopping_and_starting_streams_exactly_the_frames_it_was_given() {
        let spec = video::VideoSpec {
            pace: video::Pace::Emulated,
            ..video::VideoSpec::new(video::Destination::Stdout)
        };
        let Some(mut app) = test_app_video(
            ProtocolType::Kitty,
            RenderPath::Fast,
            VideoOut::in_memory(spec),
        ) else {
            return;
        };
        for cycle in 1..=10u64 {
            // Off: no *game* frame may be written, however many arrive.
            app.video.go_idle();
            for _ in 0..5 {
                app.step_frame();
            }
            assert_eq!(
                app.video.stats().frames,
                (cycle - 1) * 3,
                "cycle {cycle}: the game went out while the feed was off"
            );
            // On: three frames in, three frames out.
            app.video.resume();
            for _ in 0..3 {
                app.step_frame();
            }
            assert_eq!(
                app.video.stats().frames,
                cycle * 3,
                "cycle {cycle}: frame counter drifted"
            );
        }
    }

    /// **The captain's acceptance test, headless.** Stopping the feed must not
    /// leave the consumer frozen on the last game frame — his Meet call sat on
    /// a title screen while the emulator was in a Rattata battle, which looks
    /// live and is not. So an idling destination keeps being written to, with
    /// the card, and the status row says which.
    #[test]
    fn stopping_the_feed_sends_the_idle_card_rather_than_freezing_the_consumer() {
        let spec = video::VideoSpec {
            pace: video::Pace::Emulated,
            ..video::VideoSpec::new(video::Destination::Path(PathBuf::from(
                "/dev/video42",
            )))
        };
        let Some(mut app) = test_app_video(
            ProtocolType::Kitty,
            RenderPath::Fast,
            VideoOut::in_memory(spec),
        ) else {
            return;
        };
        app.step_frame();
        assert_eq!(app.video.state(), video::StreamState::Live);
        let game = app.video.stats().frames;

        app.video.go_idle();
        assert!(!app.video.is_enabled(), "idling is not streaming the game");
        assert_eq!(app.video.state(), video::StreamState::Off);
        assert!(app.video.is_idle());
        assert_eq!(app.video_status(), "◍ idle video42");

        // The frontend's per-frame tick, over a second of wall clock.
        let t0 = Instant::now();
        let mut n = 0;
        while n < 120 {
            app.video.idle(t0 + Duration::from_millis(n * 10));
            n += 1;
        }
        let s = app.video.stats();
        assert_eq!(s.frames, game, "the game must not go out while off");
        assert!(
            s.idle_frames >= 2,
            "a held destination must keep receiving something — got {} cards \
             over 1.2 s",
            s.idle_frames
        );
        // …and it is the card, not a stale game frame: the card fills the
        // frame, so its first pixel is the card's background.
        let frame = 160 * 144 * 4;
        let last = &app.video.captured()[app.video.captured().len() - frame..];
        assert_eq!(
            &last[..4],
            &video::card::DMG_DARKEST,
            "the last thing sent was not the idle card"
        );

        app.video.resume();
        app.step_frame();
        assert_eq!(app.video.stats().frames, game + 1, "the game came back");
        assert_eq!(app.video.state(), video::StreamState::Live);
    }

    /// **`v` is the live feed, `V` is a recording, and they never trade
    /// places.** Pressing `v` with nothing configured used to quietly record a
    /// file, so "stop the stream" answered `REC stopped: … → ~/Videos/gameboy`
    /// to somebody thinking about his camera.
    #[test]
    fn the_v_key_never_silently_records_a_file() {
        let dir = std::env::temp_dir()
            .join(format!("gameboy-vid-test/{}-intents", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.config.video_dir = Some(dir.to_string_lossy().into_owned());

        app.toggle_video();
        assert!(
            !app.video.is_enabled(),
            "`v` with no live destination must not start anything"
        );
        let toast = app
            .toast
            .as_ref()
            .map(|(t, _)| t.clone())
            .unwrap_or_default();
        assert!(toast.contains("video_out"), "{toast}");
        assert!(toast.contains("press V"), "{toast}");
        assert!(!dir.exists(), "`v` created a recordings directory");

        // `V` still records, and says REC.
        app.toggle_recording();
        assert!(app.video.is_enabled(), "toast: {:?}", app.toast);
        assert!(
            app.video_status().starts_with("● REC"),
            "{}",
            app.video_status()
        );
        app.toggle_recording();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `video_source = display` carries the palette and the upscaler, i.e.
    /// what the player is actually looking at.
    #[test]
    fn the_display_source_streams_the_upscaled_palette_mapped_picture() {
        let spec = video::VideoSpec {
            pace: video::Pace::Emulated,
            source: video::Source::Display,
            size: Some((320, 288)),
            ..video::VideoSpec::new(video::Destination::Stdout)
        };
        let Some(mut app) = test_app_video(
            ProtocolType::Kitty,
            RenderPath::Fast,
            VideoOut::in_memory(spec),
        ) else {
            return;
        };
        app.upscaler
            .set_algorithm(crate::upscale::Algorithm::Scale2x);
        app.upscale_factor = 2;
        for _ in 0..2 {
            app.step_frame();
        }
        assert_eq!(app.video.geometry(), Some((320, 288)));
        assert_eq!(app.video.captured().len(), 2 * 320 * 288 * 4);
        // Green palette: the emulator's greys are gone from the stream.
        let f = &app.video.captured()[..320 * 288 * 4];
        assert!(
            f.chunks_exact(4).any(|p| p[0] != p[1] || p[1] != p[2]),
            "a palette-mapped stream must not still be greyscale"
        );
    }

    // -----------------------------------------------------------------------
    // Graceful degradation: an ordinary terminal must still run the emulator.
    //
    // `8357546` added a hard `return Err` at the top of `run` when the terminal
    // reported no kitty keyboard protocol, so GNOME Terminal — the captain's
    // default — printed an error and exited having drawn nothing. These tests
    // are the ones that would have caught it.
    // -----------------------------------------------------------------------

    fn key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        use ratatui::crossterm::event::{KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        }
    }

    /// A plain terminal reports every key as `Press` — no `Repeat`, no
    /// `Release`. That is the whole difference the fallback has to cover.
    fn basic_press(code: KeyCode) -> KeyEvent {
        key(code, KeyEventKind::Press)
    }

    fn is_down(app: &App, button: KeypadKey) -> bool {
        app.session.as_ref().expect("session").held[key_index(button)].is_some()
    }

    #[test]
    fn a_terminal_without_keyboard_enhancement_is_not_a_refusal() {
        // The regression, stated directly: no answer from this function may be
        // a refusal, and only a clear "yes" earns the enhanced path.
        assert_eq!(input_mode_for(Ok(true)), InputMode::Enhanced);
        assert_eq!(input_mode_for(Ok(false)), InputMode::Basic);
        assert_eq!(
            input_mode_for(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no reply"
            ))),
            InputMode::Basic,
            "an unanswered question is not a reason to refuse either"
        );
    }

    #[test]
    fn refusal_only_when_there_is_neither_a_screen_nor_a_keyboard() {
        assert!(terminal_is_usable(true, true));
        assert!(terminal_is_usable(true, false), "output alone is enough");
        assert!(terminal_is_usable(false, true), "input alone is enough");
        assert!(!terminal_is_usable(false, false));
        // And the refusal has to say what to do instead, or it is the old bug
        // wearing a different message.
        let msg = refusal_message();
        assert!(msg.contains("desktop"), "{msg}");
    }

    #[test]
    fn the_status_panel_says_which_input_mode_is_live() {
        use ratatui::backend::TestBackend;
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        for (mode, want) in [(InputMode::Enhanced, "kitty"), (InputMode::Basic, "basic")]
        {
            app.input = mode;
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(
                text.contains("Input") && text.contains(want),
                "the input mode must be visible on screen; wanted {want:?}"
            );
        }
    }

    #[test]
    fn a_basic_terminal_holds_a_button_across_auto_repeat() {
        // X11's default auto-repeat: 500 ms delay, then 30/s. Nothing but those
        // repeats says the key is still down.
        let Some(mut app) = test_app(ProtocolType::Halfblocks, RenderPath::Fast) else {
            return;
        };
        app.input = InputMode::Basic;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        app.handle_game_key_at(basic_press(KeyCode::Right), at(0));
        assert!(is_down(&app, KeypadKey::Right));

        // The auto-repeat *delay* is the honest cost: the button is let go
        // before the first repeat arrives, so a held direction takes one step
        // and pauses. Nothing in a plain terminal can distinguish that from a
        // tap, which is why we do not pretend otherwise.
        app.release_expired_at(at(300));
        assert!(!is_down(&app, KeypadKey::Right));

        // From the first repeat on, it must stay down continuously — a button
        // that flickered between repeats would make walking impossible.
        for n in 0..30u64 {
            let t = 500 + n * 33;
            app.handle_game_key_at(basic_press(KeyCode::Right), at(t));
            app.release_expired_at(at(t + 16)); // one emulated frame later
            assert!(
                is_down(&app, KeypadKey::Right),
                "released mid-auto-repeat at {t} ms"
            );
        }
        // …and let go promptly once the repeats stop.
        app.release_expired_at(at(500 + 29 * 33 + 400));
        assert!(!is_down(&app, KeypadKey::Right));
    }

    #[test]
    fn the_hold_window_adapts_to_a_slow_auto_repeat_rate() {
        // A user with a 5/s repeat rate (200 ms) would flicker on the 160 ms
        // starting window. One observed gap must fix that permanently.
        let Some(mut app) = test_app(ProtocolType::Halfblocks, RenderPath::Fast) else {
            return;
        };
        app.input = InputMode::Basic;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let idx = key_index(KeypadKey::Left);
        let window = |a: &App| {
            a.session
                .as_ref()
                .unwrap()
                .hold_window(InputMode::Basic, idx)
        };

        assert_eq!(window(&app), KEY_HOLD_BASIC);
        for n in 0..4u64 {
            app.handle_game_key_at(basic_press(KeyCode::Left), at(500 + n * 200));
        }
        let after = window(&app);
        assert!(
            after > Duration::from_millis(200),
            "window {after:?} must now bridge a 200 ms repeat interval"
        );
        assert!(after <= KEY_HOLD_BASIC_MAX, "and stay bounded: {after:?}");

        // The auto-repeat *delay* (the 500 ms first gap) must not be mistaken
        // for the rate — learning it would leave a tap walking for a second.
        let mut app2 = test_app(ProtocolType::Halfblocks, RenderPath::Fast).unwrap();
        app2.input = InputMode::Basic;
        app2.handle_game_key_at(basic_press(KeyCode::Left), at(0));
        app2.handle_game_key_at(basic_press(KeyCode::Left), at(500));
        assert_eq!(
            window(&app2),
            KEY_HOLD_BASIC,
            "a 500 ms gap is the repeat delay, not the repeat rate"
        );
    }

    #[test]
    fn auto_repeat_cannot_strobe_a_one_shot_control_key() {
        // Holding `[` in a plain terminal delivers ~30 presses a second. Each
        // one used to cycle the palette, which looks like the emulator having a
        // fit; the same stream on `v` would flap the video stream on and off.
        let Some(mut app) = test_app(ProtocolType::Halfblocks, RenderPath::Fast) else {
            return;
        };
        app.input = InputMode::Basic;
        let t0 = Instant::now();
        let start = app.palette;
        // One press, then the auto-repeat stream.
        app.handle_game_key_at(basic_press(KeyCode::Char('[')), t0);
        let after_first = app.palette;
        assert_ne!(after_first, start, "the real press must still act");
        for n in 1..40u64 {
            app.handle_game_key_at(
                basic_press(KeyCode::Char('[')),
                t0 + Duration::from_millis(500 + n * 33),
            );
        }
        // The 500 ms first repeat is indistinguishable from a deliberate second
        // press, so it is allowed through; the 30/s stream behind it is not.
        assert_eq!(
            app.palette,
            after_first.next(),
            "auto-repeat must not strobe the palette"
        );

        // A genuine second press, well clear of the lockout, still works.
        app.handle_game_key_at(
            basic_press(KeyCode::Char('[')),
            t0 + Duration::from_millis(4000),
        );
        assert_eq!(app.palette, after_first.next().next());
    }

    #[test]
    fn the_kitty_path_is_unchanged_by_the_fallback() {
        // WezTerm/Kitty must take exactly the path they took before: `Repeat`
        // refreshes the hold, `Release` ends it immediately, and no lockout
        // touches control keys.
        let Some(mut app) = test_app(ProtocolType::Halfblocks, RenderPath::Fast) else {
            return;
        };
        app.input = InputMode::Enhanced;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        app.handle_game_key_at(key(KeyCode::Up, KeyEventKind::Press), at(0));
        app.handle_game_key_at(key(KeyCode::Up, KeyEventKind::Repeat), at(50));
        app.release_expired_at(at(60));
        assert!(is_down(&app, KeypadKey::Up));
        app.handle_game_key_at(key(KeyCode::Up, KeyEventKind::Release), at(70));
        assert!(
            !is_down(&app, KeypadKey::Up),
            "a real release must be immediate, not a timeout"
        );

        // Two rapid presses of a control key both act (no lockout here).
        let start = app.palette;
        app.handle_game_key_at(key(KeyCode::Char('['), KeyEventKind::Press), at(80));
        app.handle_game_key_at(key(KeyCode::Char('['), KeyEventKind::Press), at(90));
        assert_eq!(app.palette, start.next().next());
    }

    /// `docs/plugins.md` §4 promises "a plugin cannot be active and invisible".
    /// The Status box used to be a fixed 13 rows, which fits exactly eleven
    /// content lines — and the Plugin row is pushed *last*, so it was clipped
    /// and an attached plugin left no trace on screen. Asserting the row's text
    /// is not enough; it has to be asserted **in rendered output**.
    #[test]
    fn an_attached_plugin_is_visible_in_the_rendered_status_panel() {
        use ratatui::backend::TestBackend;

        /// Minimal plugin: the point is the row, not what it says.
        struct Marker;
        impl crate::plugin::Plugin for Marker {
            fn name(&self) -> &str {
                "zzmarker"
            }
            fn capabilities(&self) -> crate::plugin::Capability {
                crate::plugin::Capability::OBSERVE
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        let s = app.session.as_mut().expect("session");
        s.gb.add_plugin(Box::new(Marker)).expect("attach");

        // Sizes tall enough to have a sidebar at all.
        for (w, h) in [(120u16, 40u16), (100, 30), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(
                text.contains("zzmarker"),
                "an attached plugin must be visible at {w}x{h}"
            );
            assert!(
                text.contains("trainer"),
                "and so must every *other* attached plugin at {w}x{h} — one \
                 row each, never one truncated joined line"
            );
        }
    }

    /// The status panel must name **both** halves of a displayed frame: the
    /// pictures we issued, and what the terminal costs to take one.
    ///
    /// One number alone is how "reports 59 fps but is clearly rendering slower"
    /// stayed unexplained — `fps` is entirely ours and cannot go down for
    /// anything the terminal does. And when the terminal cannot be asked, the
    /// panel must say so rather than print a plausible zero.
    #[test]
    fn the_status_panel_reports_the_terminal_alongside_our_own_frame_rate() {
        use ratatui::backend::TestBackend;
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        // No probe ran in a test binary, so the terminal is unmeasurable — and
        // that is exactly the case that must not be dressed up as a number.
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("FPS"), "our own rate is missing");
        assert!(text.contains("Term"), "the terminal's rate is missing");
        assert!(
            text.contains("n/a"),
            "an unmeasurable terminal must say so, not print a number: {text}"
        );
    }

    /// The transmitted image is the source resolution whatever the window does.
    ///
    /// This is the rule `docs/terminal-render.md` was written to establish, and
    /// it is worth a standing test because the failure mode is invisible from
    /// our side: the payload stays small in *our* timings while the terminal
    /// quietly composites something enormous.
    #[test]
    fn the_transmitted_image_is_the_source_resolution_at_every_window_size() {
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        for (w, h) in [(60u16, 20u16), (120, 40), (213, 55), (268, 60), (400, 120)] {
            let _ = drawn_bytes(&mut app, w, h);
            let s = app.session.as_ref().expect("session");
            assert_eq!(
                s.tx_px,
                (160, 144),
                "transmitted {:?} at {w}x{h} — the terminal must do the scaling",
                s.tx_px
            );
        }
    }

    /// `cell_scale` cancels out of the cell rectangle, so it cannot make the
    /// picture we ask the terminal to composite any larger.
    ///
    /// Worth pinning because the HiDPI note in `AGENTS.md` reads as though
    /// `cell_scale = auto` doubles something that reaches the wire. It does not:
    /// it scales the *font*, and the fit divides by the same font, so the cell
    /// rectangle is identical and the payload is 160×144 either way. All it
    /// changes is the pixel figure the status panel reports.
    #[test]
    fn cell_scale_does_not_change_what_the_terminal_is_asked_to_composite() {
        let area = Rect::new(0, 0, 238, 58);
        let (plain, s1) = fit_metrics(area, 160, 144, 19, 45);
        let (doubled, s2) = fit_metrics(area, 160, 144, 38, 90);
        assert_eq!(
            plain, doubled,
            "cell_scale changed the composited rectangle: {plain:?} vs {doubled:?}"
        );
        // The reported scale doubles — that is the *only* thing it moves, and
        // it is a number on a panel, not bytes or pixels.
        assert!((s2 - s1 * 2.0).abs() < 0.01, "{s1} {s2}");
    }

    /// The volume keys, driven through the real dispatcher.
    ///
    /// A control the player cannot find does not exist — this emulator's own
    /// history is a list of features somebody had to be *told* about — so the
    /// binding and the feedback are pinned together: every press says what it
    /// did, and mute says how to undo itself.
    #[test]
    fn the_volume_keys_are_bound_and_report_what_they_did() {
        let _g = output_volume::test_lock();
        isolate_state_dir();
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        let press = |app: &mut App, code: KeyCode| {
            app.toast = None;
            app.handle_key(KeyEvent::new(code, event::KeyModifiers::NONE));
            app.toast_text().map(str::to_string)
        };

        for code in [KeyCode::Char(','), KeyCode::Char('<')] {
            let msg = press(&mut app, code).unwrap_or_default();
            assert!(msg.starts_with("Volume "), "{code:?} said {msg:?}");
        }
        for code in [KeyCode::Char('.'), KeyCode::Char('>')] {
            let msg = press(&mut app, code).unwrap_or_default();
            assert!(msg.starts_with("Volume "), "{code:?} said {msg:?}");
        }
        // Mute names its own escape route: "there is no sound" must never be a
        // state the player cannot get out of.
        let msg = press(&mut app, KeyCode::Char('m')).unwrap_or_default();
        assert!(msg.contains("Muted") && msg.contains('m'), "{msg:?}");
        let msg = press(&mut app, KeyCode::Char('m')).unwrap_or_default();
        assert!(msg.starts_with("Volume "), "unmute said {msg:?}");

        // A player who bound `m` to a Game Boy button keeps their binding —
        // their configuration outranks ours.
        app.config.keys.b = 'm';
        let before = output_volume::muted();
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(
            output_volume::muted(),
            before,
            "`key_b = m` must still mean B"
        );
    }

    /// Volume has to be *visible*: the level in the Status panel, the keys in
    /// the Controls list, and mute called out on the top bar — a muted emulator
    /// and a broken one look identical otherwise.
    #[test]
    fn the_volume_control_is_visible_on_screen() {
        use ratatui::backend::TestBackend;
        let _g = output_volume::test_lock();
        isolate_state_dir();
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        let screen = |app: &mut App| -> String {
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect()
        };

        let text = screen(&mut app);
        assert!(
            text.contains("Volume"),
            "the Status panel must show the level"
        );
        assert!(
            text.contains(", . volume"),
            "the Controls list must name the keys"
        );
        assert!(text.contains("m mute"), "including mute");

        // Muted is spelled out, not shown as a plausible number.
        output_volume::set_muted(true);
        let text = screen(&mut app);
        assert!(
            text.contains("MUTED"),
            "mute must be unmissable on the top bar and in Status"
        );
        output_volume::set_muted(false);
    }

    /// The trainer panel's own keys, driven through the real dispatcher.
    ///
    /// `d` and `Tab` are guarded on the panel being up (`if self.trainer`) so
    /// they stay free as game buttons the rest of the time, and that guard is
    /// exactly the kind of thing that silently stops working. **In a battle Tab
    /// starts at the opponent**, which is the whole point of the ring.
    #[test]
    fn the_trainer_keys_toggle_detail_and_walk_the_focus_ring() {
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        let press = |app: &mut App, code: KeyCode| {
            app.handle_key(KeyEvent::new(code, event::KeyModifiers::NONE));
        };
        // With the panel down, `d` is not the panel's key.
        app.trainer = false;
        app.trainer_detail = false;
        press(&mut app, KeyCode::Char('d'));
        assert!(
            !app.trainer_detail,
            "`d` must not bind while the panel is down"
        );
        app.trainer = true;
        press(&mut app, KeyCode::Char('d'));
        assert!(app.trainer_detail, "`d` opens the detail view");
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.trainer_detail, "and closes it again");

        // The ring: with no trainer data the selection cannot move, and with a
        // ring of N it wraps rather than running off the end.
        app.trainer_sel = 0;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.trainer_sel, 0, "an empty ring does not move");
    }

    /// The ring itself, over states rather than over a running cartridge: in a
    /// battle position 0 is the opponent, so one `Tab` from a fresh battle
    /// reaches the party and the sprite follows the focus rather than being
    /// pinned to the opponent.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn the_focus_ring_puts_the_opponent_first_in_a_battle() {
        use terminalgb_plugin_trainer::gen1::{BattleKind, Mon};
        use terminalgb_plugin_trainer::TrainerState;
        let mut st = TrainerState {
            in_game: true,
            ..Default::default()
        };
        st.party.push(Mon::default());
        st.party.push(Mon::default());
        // Out of battle the ring is the party alone.
        assert_eq!(tv::focus_ring(&st).len(), 2);
        assert_eq!(tv::focus_at(&st, 0), Some(tv::Focus::Party(0)));
        st.battle = BattleKind::Wild;
        st.opponent = Some(Mon::default());
        assert_eq!(tv::focus_ring(&st).len(), 3);
        assert_eq!(tv::focus_at(&st, 0), Some(tv::Focus::Opponent));
        assert_eq!(tv::focus_at(&st, 1), Some(tv::Focus::Party(0)));
        // A stale index from a longer ring clamps rather than vanishing.
        assert_eq!(tv::focus_at(&st, 99), Some(tv::Focus::Party(1)));
    }

    /// The panel shrinks with the window instead of squeezing the game out,
    /// and stands down entirely when there is not enough left to be worth the
    /// space. This is what makes `trainer_view`'s narrow tiers reachable — they
    /// were unreachable while the sidebar was a fixed 40 columns.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn the_trainer_panel_gives_width_back_to_the_game() {
        use ratatui::backend::TestBackend;
        for cols in [40u16, 50, 60, 70, 80, 120, 200] {
            let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
                return;
            };
            app.trainer = true;
            let mut terminal = Terminal::new(TestBackend::new(cols, 40)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let s = app.session.as_ref().unwrap();
            assert!(
                s.disp_cells.0 >= MIN_GAME_W.min(cols),
                "{cols}: the picture was squeezed to {} cells",
                s.disp_cells.0
            );
            if let Some(panel) = app.trainer_rect {
                // The panel keeps a proportion of the window rather than a
                // fixed column count — that is what stops it reading as a
                // footnote beside a picture that grows.
                assert!(
                    panel.width <= cols / 2 || panel.width == tv::PANEL_MIN_W,
                    "{cols}: panel took {} of {cols}",
                    panel.width
                );
                assert!(
                    panel.width >= tv::PANEL_MIN_W,
                    "{cols}: a panel too narrow to read was drawn"
                );
                assert!(panel.right() <= cols, "{cols}: panel escapes the window");
            }
            // The proportion actually holds: at every size that has room for
            // both, the panel is between a third and a half of the width.
            if cols >= 60 {
                let panel = trainer_panel_width(cols);
                assert!(
                    panel * 3 >= cols && panel * 2 <= cols,
                    "{cols}: panel {panel} is out of proportion"
                );
            }
        }
    }

    /// "What's the shortcut for putting the side panel in the stream?" had no
    /// answer anywhere in the product — not that it was unsupported, not where
    /// to go instead. Asserted against **rendered** output, because a line in
    /// the source that the overlay clips away helps nobody.
    #[test]
    fn the_help_says_the_feed_is_game_only_and_where_to_go_instead() {
        use ratatui::backend::TestBackend;
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.overlay = Overlay::Help;
        let (cols, rows) = (140u16, 60u16);
        let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let screen: String = (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        // Newlines are wrapping points, so match on words rather than phrases.
        let flat: String = screen.split_whitespace().collect::<Vec<_>>().join(" ");
        for want in ["GAME PICTURE ONLY", "OBS", "TerminalGB"] {
            assert!(
                flat.contains(want),
                "the help overlay never says {want:?}:\n{screen}"
            );
        }
    }

    /// The window is named so an OBS scene can find it — the emulator-side half
    /// of the window-capture route.
    #[test]
    fn the_window_title_keeps_a_stable_prefix() {
        assert_eq!(TERM_TITLE, "TerminalGB");
        // A title with a game in it must still start with the prefix an OBS
        // source matches on, or a scene breaks on every ROM switch.
        for game in [None, Some("POKEMON BLUE"), Some("  "), Some("")] {
            let t = window_title(game);
            assert!(t.starts_with(TERM_TITLE), "{t}");
        }
        assert_eq!(window_title(None), TERM_TITLE);
        assert_eq!(window_title(Some("  ")), TERM_TITLE, "a blank header title");
        assert_eq!(
            window_title(Some("POKEMON BLUE")),
            "TerminalGB — POKEMON BLUE"
        );
    }

    /// The bar is one row, never wraps, never clips, and **never loses the
    /// plugins or the controls entirely** — those are the two things it exists
    /// to make visible.
    #[test]
    fn the_top_bar_fits_one_row_at_every_width() {
        use ratatui::backend::TestBackend;
        for cols in [30u16, 40, 50, 62, 80, 100, 140, 200] {
            let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
                return;
            };
            app.trainer = true;
            let mut terminal = Terminal::new(TestBackend::new(cols, 30)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let row: String = (0..cols)
                .map(|x| buf.cell((x, 0)).map(|c| c.symbol()).unwrap_or(" "))
                .collect();
            assert!(
                row.chars().count() == cols as usize,
                "{cols}: the bar is not exactly one row wide"
            );
            // Identity survives everywhere; so does a way into the help.
            assert!(
                row.contains("ACID2") || row.contains('?'),
                "{cols}: {row:?}"
            );
            // Wide enough to say anything at all, and the two things the bar
            // exists for are both on it.
            if cols >= 62 {
                assert!(row.contains("trainer"), "{cols}: no plugin state: {row:?}");
                assert!(row.contains("? help"), "{cols}: no controls: {row:?}");
            }
        }
    }

    /// The bar replaces the title row and frees the bottom one, so it costs no
    /// rows — which matters because it was asked for in a pane where vertical
    /// space is the scarce thing.
    #[test]
    fn the_top_bar_costs_no_rows() {
        use ratatui::backend::TestBackend;
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.trainer = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let last: String = (0..100)
            .map(|x| buf.cell((x, 39)).map(|c| c.symbol()).unwrap_or(" "))
            .collect();
        assert!(
            last.trim().is_empty(),
            "the bottom row should be free unless a toast is up: {last:?}"
        );
    }

    /// Segments shrink through their own ladders before any is dropped, so a
    /// narrow bar carries a short control hint rather than none — and never a
    /// clipped one, which the reader cannot tell from a complete one.
    #[test]
    fn the_bar_shortens_before_it_drops() {
        let ladders = vec![
            vec!["TITLE".to_string()],
            vec!["a-very-long-plugin-detail".into(), "plug".into()],
            vec!["? help · p pause · q quit".into(), "? help".into()],
        ];
        let wide = fit_bar(&ladders, "  ", 80);
        assert_eq!(wide[1], "a-very-long-plugin-detail");
        assert_eq!(wide[2], "? help · p pause · q quit");
        let mid = fit_bar(&ladders, "  ", 24);
        assert_eq!(mid[0], "TITLE");
        assert!(!mid[1].is_empty() && !mid[2].is_empty(), "{mid:?}");
        assert!(
            mid.iter().filter(|s| !s.is_empty()).count() == 3,
            "shorten before dropping: {mid:?}"
        );
        // Only when nothing can shrink further does a segment go, from the
        // right, and the first one never does.
        let tiny = fit_bar(&ladders, "  ", 8);
        assert_eq!(tiny[0], "TITLE");
        assert_eq!(tiny[2], "", "the rightmost goes first");
    }

    /// The picture and the panel are **one composition**: adjacent, sharing a
    /// vertical extent, centred together. Three separate complaints came from
    /// this not holding — the picture floating while the panel stuck to the
    /// ceiling, then the reverse, and a band of dead space between the two.
    #[test]
    fn the_picture_and_the_panel_are_one_block() {
        use ratatui::backend::TestBackend;
        for (cols, rows) in [
            (62u16, 48u16),
            (80, 30),
            (100, 34),
            (120, 40),
            (140, 44),
            (200, 60),
            (62, 24),
        ] {
            let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
                return;
            };
            app.trainer = true;
            let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let Some(panel) = app.trainer_rect else {
                continue;
            };
            let s = app.session.as_ref().unwrap();
            let pic = Rect::new(
                s.disp_origin.0,
                s.disp_origin.1,
                s.disp_cells.0,
                s.disp_cells.1,
            );
            // Adjacent: no band of dead space between them. The picture's own
            // column may be wider than the picture (it is centred inside it),
            // so the test is that the gap is at most that centring slack.
            assert!(
                panel.x >= pic.right() && panel.x - pic.right() <= 1,
                "{cols}x{rows}: {} columns of dead space between picture {pic:?} \
                 and panel {panel:?}",
                panel.x.saturating_sub(pic.right())
            );
            // One vertical alignment: the two are centred about the same line.
            let mid = |r: Rect| r.y as i32 * 2 + r.height as i32;
            assert!(
                (mid(pic) - mid(panel)).abs() <= 2,
                "{cols}x{rows}: picture {pic:?} and panel {panel:?} do not share \
                 a vertical centre"
            );
        }
    }

    /// A crashed game stops the emulator writing the save file. The player has
    /// to be able to *see* that, for the same reason an attached plugin does —
    /// and the row is pushed last, exactly where the clipping bug above lived.
    #[test]
    fn a_faulted_machine_says_so_in_the_rendered_status_panel() {
        use ratatui::backend::TestBackend;

        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.session
            .as_mut()
            .expect("session")
            .gb
            .report_fault("the CPU left the program");

        for (w, h) in [(120u16, 40u16), (100, 30), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(
                text.contains("SAVE NOT WRITTEN"),
                "a machine that has stopped saving must say so at {w}x{h}"
            );
        }
    }

    /// Centring only helps while there is slack. When the panel wants more rows
    /// than the pane has, it must fill the pane from the **top** — pushing the
    /// header off the ceiling to centre an over-height panel would lose the
    /// part that says whose panel it is.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn an_over_height_panel_keeps_its_head_and_loses_its_tail() {
        let area = Rect::new(0, 0, tv::PANEL_W, 12);
        let l = tv::layout_with_team(area, true, false, 40).centre_in(area, 40);
        assert_eq!(l.frame, area, "no slack: the panel fills the column");
        assert_eq!(l.header.y, area.y + 1, "and the header stays at the top");
        // With slack it centres instead.
        let tall = Rect::new(0, 0, tv::PANEL_W, 60);
        let c = tv::layout_with_team(tall, true, false, 10).centre_in(tall, 10);
        assert!(
            c.frame.y > tall.y,
            "with room to spare the panel is centred"
        );
        assert_eq!(
            c.frame.y - tall.y,
            tall.bottom() - c.frame.bottom(),
            "and the slack is even"
        );
    }

    /// The other half of the same bug: the stats panel must not occupy a cell
    /// the picture occupies, **at any window size**. A terminal image owns whole
    /// cells, so an overlap is not "text on top of an image" — it is text
    /// *instead of* it, and for Kitty's placeholder placement it removes the
    /// cells that make the picture update at all.
    #[test]
    fn the_stats_panel_never_overlaps_the_picture_at_any_size() {
        use ratatui::backend::TestBackend;
        let sizes = [
            (60u16, 20u16),
            (80, 24),
            (120, 40),
            (160, 50),
            (200, 60),
            (320, 90),
            (240, 30),
            (100, 80),
        ];
        for fullscreen in [false, true] {
            for (cols, rows) in sizes {
                let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
                app.stats = true;
                app.fullscreen = fullscreen;
                let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
                terminal.draw(|f| app.render(f)).unwrap();
                let s = app.session.as_ref().unwrap();
                let picture = Rect::new(
                    s.disp_origin.0,
                    s.disp_origin.1,
                    s.disp_cells.0,
                    s.disp_cells.1,
                );
                let stats = app.stats_rect.expect("stats panel not drawn");
                assert!(
                    picture.intersection(stats).area() == 0,
                    "{cols}x{rows} fullscreen={fullscreen}: stats {stats:?} overlaps \
                     picture {picture:?} — that blanks the game and, with kitty \
                     placeholders, freezes it"
                );
                assert!(picture.area() > 0, "{cols}x{rows}: no picture drawn at all");
            }
        }
    }

    /// The commentary panel is the third thing that can land on the picture,
    /// and the one most likely to: it is full width and it is on by default
    /// whenever an agent is attached.
    ///
    /// The sizes include the ones an OBS layout actually produces — a 1920x1080
    /// canvas at a stream-legible font is roughly 100-140 columns by 26-34
    /// rows — as well as the small windows a developer uses.
    #[test]
    fn the_commentary_panel_never_overlaps_the_picture() {
        use ratatui::backend::TestBackend;
        let sizes = [
            (60u16, 20u16),
            (80, 24),
            (100, 26),
            (106, 30),
            (120, 34),
            (140, 30),
            (160, 50),
            (200, 60),
            (40, 12),
        ];
        for fullscreen in [false, true] {
            for trainer in [false, true] {
                for (cols, rows) in sizes {
                    let mut app =
                        test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
                    app.agent_panel = true;
                    app.trainer = trainer;
                    app.fullscreen = fullscreen;
                    let mut terminal =
                        Terminal::new(TestBackend::new(cols, rows)).unwrap();
                    terminal.draw(|f| app.render(f)).unwrap();
                    let s = app.session.as_ref().unwrap();
                    let picture = Rect::new(
                        s.disp_origin.0,
                        s.disp_origin.1,
                        s.disp_cells.0,
                        s.disp_cells.1,
                    );
                    if let Some(panel) = app.agent_rect {
                        assert_eq!(
                            picture.intersection(panel).area(),
                            0,
                            "{cols}x{rows} fullscreen={fullscreen} trainer={trainer}: \
                             commentary {panel:?} overlaps picture {picture:?}"
                        );
                        if let Some(t) = app.trainer_rect {
                            assert_eq!(
                                t.intersection(panel).area(),
                                0,
                                "{cols}x{rows}: commentary overlaps the trainer panel"
                            );
                        }
                    }
                    assert!(
                        picture.area() > 0,
                        "{cols}x{rows}: the panel squeezed the picture out entirely"
                    );
                }
            }
        }
    }

    /// "A plugin cannot be active and invisible" now has a second thing to be
    /// visible about: something else is **pressing your buttons**. Asserted in
    /// rendered output, because the row's text being right is not the bug that
    /// has bitten this panel three times — the row being clipped away is.
    #[test]
    fn a_plugin_that_presses_buttons_is_marked_in_the_rendered_status_panel() {
        use ratatui::backend::TestBackend;

        struct Presser(bool);
        impl crate::plugin::Plugin for Presser {
            fn name(&self) -> &str {
                "zzdriver"
            }
            fn capabilities(&self) -> crate::plugin::Capability {
                crate::plugin::Capability::OBSERVE
                    | crate::plugin::Capability::CONTROL_INPUT
            }
            fn on_event(
                &mut self,
                _e: &crate::plugin::Event,
                m: &mut crate::plugin::Machine,
            ) {
                if !self.0 {
                    self.0 = true;
                    m.tap(crate::input::KeypadKey::A);
                }
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        let render = |app: &mut App, w: u16, h: u16| -> String {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| app.render(f)).unwrap();
            t.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect()
        };

        let s = app.session.as_mut().expect("session");
        s.gb.add_plugin(Box::new(Presser(false))).expect("attach");
        // Declared but not yet used: a single `>`.
        for (w, h) in [(120u16, 40u16), (100, 30)] {
            let text = render(&mut app, w, h);
            assert!(
                text.contains("zzdriver>"),
                "at {w}x{h} the panel must say a plugin may press buttons:\n{text}"
            );
        }
        // Actually pressed: `>>`, and it stays.
        app.session.as_mut().unwrap().gb.frame();
        let text = render(&mut app, 120, 40);
        assert!(
            text.contains("zzdriver>>"),
            "once it has pressed a button the panel must say so:\n{text}"
        );
    }

    /// What a Twitch viewer actually reads, rendered at the cell grids an OBS
    /// layout produces.
    ///
    /// A 1920x1080 canvas at a stream-legible terminal font (26-30 px, so a
    /// glyph survives Twitch's re-encode and a phone viewer's downscale) is
    /// roughly 100-140 columns by 26-34 rows — see `docs/claude-plays.md`
    /// §OBS. This asserts the things a cold joiner needs are all on screen at
    /// those sizes, and with `GB_DUMP_LAYOUT=<file>` it writes the grid out so
    /// the documented picture is a real render and not a drawing.
    #[test]
    // The commentary panel only exists when the agent plugin does — the
    // stubbed `av::split` in a build without it correctly yields no panel.
    #[cfg(feature = "plugin-agent")]
    fn the_stream_layout_shows_goal_activity_and_game_together() {
        use ratatui::backend::TestBackend;
        for (cols, rows) in [(106u16, 30u16), (120, 34), (140, 30)] {
            let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
            app.agent_panel = true;
            let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let mut grid = String::new();
            for y in 0..rows {
                for x in 0..cols {
                    grid.push_str(buf[(x, y)].symbol());
                }
                grid.push('\n');
            }
            if let Ok(path) = std::env::var("GB_DUMP_LAYOUT") {
                let _ = std::fs::write(format!("{path}.{cols}x{rows}.txt"), &grid);
            }
            // With no agent attached the panel explains itself rather than
            // being an unexplained empty box — that is the state a first-time
            // user sees, and it must not be a mystery.
            assert!(
                grid.contains("Commentary"),
                "{cols}x{rows}: no commentary panel:\n{grid}"
            );
            assert!(
                grid.contains("No agent attached"),
                "{cols}x{rows}: the empty panel must say why:\n{grid}"
            );
            // And the top bar is still there, i.e. the panel took space from
            // the layout rather than from the picture or the chrome.
            assert!(
                grid.contains("? help"),
                "{cols}x{rows}: top bar lost:\n{grid}"
            );
        }
    }

    /// The trainer panel adds a *second* terminal image (the Pokémon sprite) to
    /// a screen that already has one, so the "an image owns whole cells" rule
    /// now has to hold for two rectangles rather than one. Neither the panel's
    /// text nor its sprite may share a cell with the game picture, and the
    /// sprite may not share one with the panel's own text either.
    #[test]
    fn the_trainer_panel_and_its_sprite_never_touch_the_picture() {
        use ratatui::backend::TestBackend;
        let sizes = [
            (60u16, 20u16),
            (80, 24),
            (100, 30),
            (120, 40),
            (160, 50),
            (200, 60),
            (240, 30),
            (320, 90),
        ];
        for fullscreen in [false, true] {
            for detail in [false, true] {
                for (cols, rows) in sizes {
                    let mut app =
                        test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
                    app.trainer = true;
                    app.trainer_detail = detail;
                    app.fullscreen = fullscreen;
                    let mut terminal =
                        Terminal::new(TestBackend::new(cols, rows)).unwrap();
                    terminal.draw(|f| app.render(f)).unwrap();
                    let s = app.session.as_ref().unwrap();
                    let picture = Rect::new(
                        s.disp_origin.0,
                        s.disp_origin.1,
                        s.disp_cells.0,
                        s.disp_cells.1,
                    );
                    assert!(picture.area() > 0, "{cols}x{rows}: no picture drawn");
                    if let Some(panel) = app.trainer_rect {
                        assert_eq!(
                            picture.intersection(panel).area(),
                            0,
                            "{cols}x{rows} fullscreen={fullscreen} detail={detail}: \
                             trainer panel {panel:?} overlaps picture {picture:?}"
                        );
                    }
                    if let Some(sprite) = app.sprite_rect {
                        assert_eq!(
                            picture.intersection(sprite).area(),
                            0,
                            "{cols}x{rows}: the sprite image overlaps the game \
                             picture — two kitty images on the same cells"
                        );
                    }
                }
            }
        }
    }

    /// The shiny banner is a terminal image's worst enemy if it lands on the
    /// picture: a cell an image covers cannot also hold text. It gets its own
    /// row, at every size, in both layouts, for both verdicts.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn the_encounter_banner_never_touches_the_picture_at_any_size() {
        use ratatui::backend::TestBackend;
        let sizes = [
            (40u16, 12u16),
            (60, 20),
            (80, 24),
            (120, 40),
            (200, 60),
            (320, 90),
        ];
        for shiny in [false, true] {
            for fullscreen in [false, true] {
                for trainer in [false, true] {
                    for (cols, rows) in sizes {
                        let mut app =
                            test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
                        app.fullscreen = fullscreen;
                        app.trainer = trainer;
                        plant_encounter(&mut app, shiny);
                        assert_ne!(app.banner_state(), 0, "the banner must be up");
                        let mut terminal =
                            Terminal::new(TestBackend::new(cols, rows)).unwrap();
                        terminal.draw(|f| app.render(f)).unwrap();
                        let s = app.session.as_ref().unwrap();
                        let picture = Rect::new(
                            s.disp_origin.0,
                            s.disp_origin.1,
                            s.disp_cells.0,
                            s.disp_cells.1,
                        );
                        assert!(picture.area() > 0, "{cols}x{rows}: no picture");
                        // The banner owns row 0 in fullscreen, row 1 otherwise.
                        let banner_y = if fullscreen { 0 } else { 1 };
                        let banner = Rect::new(0, banner_y, cols, 1);
                        assert_eq!(
                            picture.intersection(banner).area(),
                            0,
                            "{cols}x{rows} full={fullscreen} trainer={trainer}: \
                             banner {banner:?} overlaps picture {picture:?}"
                        );
                    }
                }
            }
        }
    }

    /// The verdict has to reach someone who never opens the trainer panel —
    /// which is everyone, during an encounter.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn the_banner_and_the_alert_do_not_need_the_trainer_panel_open() {
        use ratatui::backend::TestBackend;
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        assert!(!app.trainer, "the panel starts closed");
        plant_encounter(&mut app, true);
        assert_eq!(app.banner_state(), 2, "a shiny verdict, panel or no panel");

        // The one-shot half: a toast and a bell, exactly once.
        app.announce_new_encounter();
        let toast = app.toast.as_ref().map(|(t, _)| t.clone()).unwrap();
        assert!(
            toast.contains("WOULD BE SHINY IF TRADED TO GEN 2"),
            "{toast}"
        );
        assert!(toast.contains("DV "), "{toast}");
        assert!(app.pending_bell, "a shiny rings the bell");
        app.pending_bell = false;
        app.toast = None;
        app.announce_new_encounter();
        assert!(app.toast.is_none(), "the same Pokémon must not re-toast");
        assert!(!app.pending_bell, "nor re-ring");

        // And the row really is drawn.
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let row: String = (0..120)
            .filter_map(|x| terminal.backend().buffer().cell((x, 1)))
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(row.contains("SHINY IF TRADED TO GEN 2"), "{row:?}");
    }

    /// The Status panel is a fixed column of rows, and rows keep being added to
    /// it (`Audio` most recently). Any of them silently clipping is the bug
    /// `an_attached_plugin_is_visible_in_the_rendered_status_panel` exists to
    /// catch, so this pins the *worst* case this crew can create: two plugins
    /// attached **and** a live wild encounter, which makes the trainer plugin's
    /// row its longest.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn every_status_row_survives_an_audio_row_and_a_live_encounter() {
        use ratatui::backend::TestBackend;
        struct Marker;
        impl crate::plugin::Plugin for Marker {
            fn name(&self) -> &str {
                "zzmarker"
            }
            fn capabilities(&self) -> crate::plugin::Capability {
                crate::plugin::Capability::OBSERVE
            }
            fn on_event(
                &mut self,
                _e: &crate::plugin::Event,
                _m: &mut crate::plugin::Machine,
            ) {
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.session
            .as_mut()
            .expect("session")
            .gb
            .add_plugin(Box::new(Marker))
            .expect("attach");
        plant_encounter(&mut app, true);

        for (w, h) in [(120u16, 40u16), (100, 30), (160, 50)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            // Every row of the Status block, first to last, plus both plugins.
            for needle in [
                "ROM", "MBC", "Scale", "Sent", "Proto", "Palette", "Upscale", "PPU",
                "Speed", "Slot", "Video", "Audio", "FPS", "trainer", "zzmarker",
            ] {
                assert!(
                    text.contains(needle),
                    "{needle:?} was pushed out of the Status panel at {w}x{h} — \
                     the box must be sized from its own row list"
                );
            }
            // And the banner is still there, in its own row.
            assert!(text.contains("SHINY IF TRADED TO GEN 2"), "at {w}x{h}");
        }
    }

    /// The verdict does not cost the Status panel a row: it rides in the
    /// trainer plugin's existing `Plugin` row, which is a second place it is
    /// visible when the panel is closed.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn a_live_encounter_rides_in_the_plugin_row_rather_than_adding_one() {
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        let rows = |app: &App| {
            app.session
                .as_ref()
                .map(|s| s.gb.plugin_statuses().len())
                .unwrap_or(0)
        };
        let before = rows(&app);
        plant_encounter(&mut app, true);
        assert_eq!(rows(&app), before, "no extra Status row");
        // The *wording* of that row is pinned next to the code that builds it,
        // in `trainer::tests::the_plugin_status_line_carries_the_verdict`: the
        // synthetic ROM here is not Red/Blue, so this plugin correctly reports
        // "unsupported cartridge" whatever is planted on it.
    }

    /// `shiny_alert = off` removes the row entirely — and with it the extra
    /// layout row, so the picture goes back to its old size.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn the_alert_can_be_switched_off() {
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        plant_encounter(&mut app, true);
        assert_eq!(app.banner_state(), 2);
        app.shiny_alert = ShinyAlert::Off;
        assert_eq!(app.banner_state(), 0);
        app.announce_new_encounter();
        assert!(!app.pending_bell);
        // `quiet` keeps the row and the toast but not the bell.
        app.shiny_alert = ShinyAlert::Quiet;
        plant_encounter(&mut app, true);
        app.announce_new_encounter();
        assert_eq!(app.banner_state(), 2);
        assert!(!app.pending_bell, "quiet means quiet");
        assert!(app.toast.is_some(), "but it still says so");
    }

    /// Plant a wild encounter on the attached plugin, so the frontend can be
    /// tested without a Pokémon cartridge.
    #[cfg(feature = "plugin-trainer")]
    fn plant_encounter(app: &mut App, shiny: bool) {
        use terminalgb_plugin_trainer::encounter::WildEncounter;
        use terminalgb_plugin_trainer::gen1::{BattleType, Dvs};
        let dvs = if shiny {
            Dvs::from_bytes(0xEA, 0xAA) // ATKDEFDV_SHINY / SPDSPCDV_SHINY
        } else {
            Dvs::from_bytes(0x9A, 0x88)
        };
        app.session
            .as_mut()
            .unwrap()
            .gb
            .plugin_mut::<TrainerPlugin>()
            .unwrap()
            .set_encounter_for_test(Some(WildEncounter {
                species: 0x24,
                dex: 16,
                name: "PIDGEY".into(),
                level: 3,
                dvs,
                kind: BattleType::Normal,
            }));
    }

    /// Switching the panel on must not cost the emulator anything when the
    /// cartridge is not a game the plugin understands — and the test ROM here
    /// is not one, so the plugin must refuse and say why.
    #[test]
    #[cfg(feature = "plugin-trainer")]
    fn an_unsupported_cartridge_refuses_the_trainer_panel() {
        use ratatui::backend::TestBackend;
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        app.toggle_trainer();
        assert!(app.trainer, "the panel still opens");
        let plugin = app
            .session
            .as_ref()
            .unwrap()
            .gb
            .plugin::<TrainerPlugin>()
            .expect("the trainer plugin is attached to every session");
        assert!(plugin.game().is_none(), "a synthetic ROM is not Red/Blue");
        assert!(plugin.refusal().is_some(), "and it explains itself");
        // It draws without panicking, and says so on screen rather than
        // showing an empty box.
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("Red/Blue"),
            "the panel must explain the refusal on screen"
        );
    }

    #[test]
    fn iterm2_places_in_cells_and_stays_small() {
        let mut app = test_app(ProtocolType::Iterm2, RenderPath::Fast).unwrap();
        let wire = drawn_wire(&mut app, 200, 60);
        let (cols, rows) = app.session.as_ref().unwrap().disp_cells;
        assert!(
            wire.contains(&format!("width={cols};height={rows}")),
            "iTerm2 image is not sized in cells"
        );
        assert!(
            !wire.contains("px;height"),
            "sized in pixels — would not scale"
        );
    }

    #[test]
    fn legacy_path_still_works_as_an_escape_hatch() {
        use ratatui::backend::TestBackend;
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Legacy).unwrap();
        assert!(app.session.as_ref().unwrap().term_image.is_none());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let bytes: usize = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().len())
            .filter(|n| *n > 1)
            .sum();
        assert!(bytes > 1000, "legacy path drew nothing");
    }

    #[test]
    fn sixel_falls_back_to_the_resize_path() {
        // Sixel has no cell-addressed placement, so the fast path must not
        // claim it.
        let app = test_app(ProtocolType::Sixel, RenderPath::Fast).unwrap();
        assert!(app.session.as_ref().unwrap().term_image.is_none());
    }

    #[test]
    fn telemetry_records_the_stage_breakdown_from_a_real_frame() {
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        app.telemetry = Telemetry::in_memory();
        app.step_frame();
        drawn_wire(&mut app, 200, 60);
        app.emit_telemetry(Duration::from_millis(3));
        let line = app.telemetry.records().first().cloned().unwrap_or_default();
        for field in [
            "\"frontend\":\"terminal\"",
            "\"protocol\":\"kitty\"",
            "\"palette\":\"green\"",
            "\"tx_w\":160",
            "\"tx_h\":144",
            "\"encode_ms\"",
            "\"upscale_ms\"",
            "\"frames_dropped\"",
        ] {
            assert!(line.contains(field), "telemetry missing {field}: {line}");
        }
        assert!(
            line.contains("\"display_px_w\":") && !line.contains("\"display_px_w\":0"),
            "display size not reported: {line}"
        );
    }

    /// The regression this whole change is about: what we transmit must be
    /// sized by the upscaler, never by the window.
    #[test]
    fn transmit_size_does_not_follow_the_window() {
        use crate::screen::config::TransmitScale;
        use crate::upscale::Algorithm;
        // `nearest` never pre-upscales for the wire — the terminal does that
        // stretch for free.
        for factor in 1..=6u8 {
            assert_eq!(
                TransmitScale::Auto.factor_for(Algorithm::Nearest, factor),
                1
            );
        }
        // A real filter transmits at its own factor, and only that.
        assert_eq!(TransmitScale::Auto.factor_for(Algorithm::Mmpx, 4), 4);
        assert_eq!(TransmitScale::Auto.factor_for(Algorithm::LcdGrid, 3), 3);
        // An explicit override is still clamped to what the algorithm can do.
        assert_eq!(TransmitScale::Fixed(3).factor_for(Algorithm::Mmpx, 2), 2);
        assert_eq!(TransmitScale::Fixed(2).factor_for(Algorithm::Nearest, 1), 2);
    }
}

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
use crate::screen::config::{CellScale, Config, Palette, Recent, RenderPath};
use crate::screen::frame::FramePrep;
use crate::screen::romlib::{self, RomEntry};
use crate::telemetry::{self, AudioSample, Context, FrameSample, Telemetry, VideoSample};
use crate::termimage::{CellRect, TermImage, Wire};
use crate::upscale::Upscaler;
use crate::video::{self, Source as VideoSource, VideoOut};

use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
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
            disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
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
/// The DMG refresh rate as a number, for "% of real hardware" reporting.
const DMG_HZ: f32 = 59.7275;
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
const STATS_H: u16 = 15;
/// A held Game Boy key auto-releases this long after its last press/repeat.
/// A safety net (we require the kitty keyboard protocol, so real key-release
/// events normally arrive first); it must exceed the terminal's key-autorepeat
/// interval so held keys don't flicker.
const KEY_HOLD: Duration = Duration::from_millis(110);

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
        .unwrap_or_else(env::temp_dir)
        .join("gameboy/states");
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
/// key, else `$XDG_VIDEOS_DIR/gameboy`, else `~/Videos/gameboy`.
fn video_dir(config: &Config) -> PathBuf {
    if let Some(d) = config.video_dir.as_ref() {
        return PathBuf::from(expand_home(d));
    }
    env::var_os("XDG_VIDEOS_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join("Videos")))
        .unwrap_or_else(env::temp_dir)
        .join("gameboy")
}

/// Where screenshots land: the `screenshot_dir` config key, else
/// `$XDG_PICTURES_DIR/gameboy`, else `~/Pictures/gameboy`, else a temp dir.
/// Somewhere a person will actually look — and the toast names the full path
/// anyway, because a screenshot that lands somewhere unknown is useless.
fn screenshot_dir(config: &Config) -> PathBuf {
    if let Some(d) = config.screenshot_dir.as_ref() {
        return PathBuf::from(expand_home(d));
    }
    env::var_os("XDG_PICTURES_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")))
        .unwrap_or_else(env::temp_dir)
        .join("gameboy")
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

/// Run the terminal frontend. `initial_rom` launches straight into that game;
/// `None` opens the ROM picker.
pub fn run(initial_rom: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    // Fail loudly on a terminal without the kitty keyboard protocol — the
    // frontend needs it for clean key up/down. (Kept from the dead-code/audio
    // crew's hardening; error returned *before* touching the terminal.)
    if let Ok(false) = ratatui::crossterm::terminal::supports_keyboard_enhancement() {
        return Err(
            "the terminal reports no support for the kitty keyboard protocol, which the \
             terminal frontend needs for key input. This is usually the protocol being \
             disabled in the terminal's config rather than truly missing — e.g. in WezTerm \
             set `enable_kitty_keyboard = true`. Terminals known to support it: Kitty, \
             WezTerm (with that setting), Ghostty, foot. Otherwise use the desktop frontend: \
             `cargo run --example desktop -- <path-to-rom.gb>`."
                .into(),
        );
    }

    let (config, warnings) = Config::load_reporting();
    let recent = Recent::load();

    // Video-out is opened *here*, before the terminal is touched, for two
    // reasons: opening a FIFO blocks until a reader attaches (which inside the
    // alternate screen looks exactly like a hang), and its notices belong in
    // the scrollback with the config ones.
    let (video, video_template, warnings) = open_video(&config, warnings);

    // Say it before the alternate screen swallows it: a config key that does
    // not apply must never fail silently. These stay in the scrollback after
    // exit, and are repeated in the picker footer and the `?` help overlay.
    for w in &warnings {
        eprintln!("gameboy: {w}");
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
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // We required kitty keyboard support above; enable event-type reporting so
    // key-release events arrive (real up/down instead of just autorepeat).
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    );

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::with_warnings(config, recent, warnings, video, video_template);
    if let Some(path) = initial_rom {
        app.start_game(&path); // a load error surfaces as a picker message
    }
    let res = app.run(&mut terminal);

    // Restore terminal.
    disable_raw_mode()?;
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

/// Open the configured raw video stream, if any, returning it plus every
/// notice worth showing the user.
///
/// Called from [`run`] **before** the terminal is put into the alternate
/// screen, because two things here are visible-side-effect-shaped: opening a
/// FIFO blocks until a reader attaches, and the `ffmpeg` command that consumes
/// the stream is the single most useful thing to leave in the scrollback.
///
/// Stdout is refused for the same reason telemetry refuses it, only harder:
/// `ratatui` writes the whole UI to stdout, so a raw frame stream there would
/// destroy both. See `examples/video_out.rs` for the stdout-pipe producer.
fn open_video(
    config: &Config,
    mut warnings: Vec<String>,
) -> (VideoOut, Option<video::VideoSpec>, Vec<String>) {
    let (spec, video_warnings) = video::VideoSpec::resolve(&config.video);
    warnings.extend(video_warnings);
    // A destination that names the game (`%t`, or a directory) cannot be
    // opened before a ROM is loaded — it is opened when the game starts.
    let template = spec
        .clone()
        .filter(|s| s.dest != video::Destination::Stdout);
    let defer = matches!(&spec, Some(s) if needs_title(&s.dest));
    if defer {
        return (VideoOut::disabled(), template, warnings);
    }
    let video = match spec {
        Some(s) if s.dest == video::Destination::Stdout => {
            warnings.push(
                "video: the terminal UI owns stdout — stream to a file or a FIFO \
                 instead (mkfifo /tmp/gb.raw; video_out = \"/tmp/gb.raw\"), or use \
                 `cargo run --release --example video_out` to pipe from a headless run"
                    .to_string(),
            );
            VideoOut::disabled()
        }
        Some(mut s) => {
            // `source = display` with no explicit size: fix the geometry from
            // the configured upscaler now, so the size announced to the
            // consumer is the truth from the first byte. Cycling the upscaler
            // mid-stream is then letterboxed into it rather than breaking a
            // running capture (see `crate::video`).
            if s.source == VideoSource::Display && s.size.is_none() {
                let up = Upscaler::new(config.upscale, config.upscale_factor);
                let (w, h) = up.output_size(160, 144);
                s.size = Some((w as u32, h as u32));
            }
            if let video::Destination::Path(p) = &s.dest {
                if is_fifo(p) {
                    eprintln!(
                        "gameboy: video: waiting for a reader on {} — start the \
                         consumer first, e.g.\n  {}",
                        p.display(),
                        s.ffmpeg_hint(None)
                    );
                }
            }
            let (v, err) = VideoOut::open(s);
            if let Some(e) = err {
                warnings.push(e);
            }
            if v.is_enabled() {
                warnings.push(v.describe());
            }
            v
        }
        None => VideoOut::disabled(),
    };
    (video, template, warnings)
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

/// Is this path a named pipe? Opening one for writing blocks until a reader
/// attaches, which deserves a word of warning rather than a silent hang.
#[cfg(unix)]
fn is_fifo(p: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(p)
        .map(|m| m.file_type().is_fifo())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_fifo(_p: &Path) -> bool {
    false
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

enum Screen {
    Picker,
    Playing,
}

#[derive(PartialEq, Eq)]
enum Overlay {
    None,
    Help,
    Quit,
}

/// The live game, present only while a ROM is loaded.
struct Session {
    gb: Gameboy,
    entry: RomEntry,
    paused: bool,
    /// Palette-mapping stage, owning its buffer so a frame allocates nothing.
    prep: FramePrep,
    /// Fast path: our own capped-resolution encoder. `None` when the protocol
    /// has no cell-addressed placement (sixel, halfblocks) or the user asked
    /// for [`RenderPath::Legacy`], in which case `image` below is used.
    term_image: Option<TermImage>,
    /// Legacy path: `ratatui-image` protocol state, rebuilt only when the frame
    /// actually changes (it used to be rebuilt every frame).
    image: Option<StatefulProtocol>,
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
    /// Per-stage timings of the most recent displayed frame, milliseconds.
    t_emulate: f32,
    t_palette: f32,
    t_upscale: f32,
    t_encode: f32,
    t_write: f32,

    /// Per-button last press time (index by [`key_index`]); drives auto-release.
    held: [Option<Instant>; 8],
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

impl Session {
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
    should_quit: bool,

    // Emulation controls (see the corresponding key handlers).
    /// Index into [`SPEED_LADDER`].
    speed: usize,
    /// Stats overlay toggle (off by default). Does not pause the game.
    stats: bool,
    /// Active save-state slot (`1..=NUM_SLOTS`).
    slot: u32,
    /// Transient in-game notice (save/load/speed feedback) with its raise time.
    toast: Option<(String, Instant)>,

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
    /// Signature of everything about the UI that can paint over the picture.
    /// When it changes, the image is retransmitted even if the frame did not,
    /// because the terminal's copy of it may have been partly overwritten.
    ui_sig: Option<(u8, bool, bool, bool)>,
    /// Machine-readable run telemetry; disabled (and free) unless configured.
    telemetry: Telemetry,
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
    /// Should the stream come back up when a game is (re)loaded? Starts true
    /// when a destination was configured, and then follows the user's last
    /// explicit `v` decision.
    video_autostart: bool,
    /// Is the live stream's destination named after the game? If so, loading a
    /// different ROM re-opens it under the new name; a plain file or FIFO the
    /// user chose keeps running across ROM switches (its consumer is attached
    /// to *that* path).
    video_named: bool,
    /// Complaints from config parsing, shown once so a mistyped key cannot
    /// fail silently.
    config_warnings: Vec<String>,
}

impl App {
    fn with_warnings(
        config: Config,
        recent: Recent,
        warnings: Vec<String>,
        video: VideoOut,
        video_template: Option<video::VideoSpec>,
    ) -> App {
        // Prefer the sharpest protocol the terminal advertises (build_picker),
        // then let an explicit config `protocol` override it.
        App::build(
            config,
            recent,
            warnings,
            build_picker(),
            video,
            video_template,
        )
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
            should_quit: false,
            speed: SPEED_NORMAL,
            stats: false,
            slot: 1,
            toast: None,
            entries: Vec::new(),
            recent_flags: Vec::new(),
            filter: String::new(),
            filtering: false,
            visible: Vec::new(),
            selected: 0,
            message: None,
            dbg_last: None,
            stats_rect: None,
            ui_sig: None,
            telemetry,
            video,
            video_prep: FramePrep::new(),
            video_upscaler: Upscaler::new(config_algo, config_factor),
            video_autostart: video_template.is_some(),
            video_named: false,
            video_template,
            config_warnings: warnings,
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

    /// Load a ROM and switch to the Playing screen. On failure, stay on the
    /// picker and show why.
    fn start_game(&mut self, path: &Path) {
        let term_image = self.new_term_image();
        match load_session(path) {
            Ok((mut gb, entry, audio)) => {
                self.recent.record(path);
                self.recent.save();
                // Carry the current speed rung's audio contract into the new
                // session (see bump_speed): non-1x must drop, not wait.
                gb.set_audio_fast_forward(self.speed != SPEED_NORMAL);
                #[cfg(feature = "audio")]
                let (audio_queue, audio_rate) = (audio.queue.clone(), audio.rate);
                self.session = Some(Session {
                    gb,
                    entry,
                    paused: false,
                    prep: FramePrep::new(),
                    term_image,
                    image: None,
                    legacy_hash: 0,
                    scale: 1.0,
                    disp_cells: (0, 0),
                    disp_origin: (0, 0),
                    disp_px: (0, 0),
                    tx_px: (160, 144),
                    tx_bytes: 0,
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
                    t_emulate: 0.0,
                    t_palette: 0.0,
                    t_upscale: 0.0,
                    t_encode: 0.0,
                    t_write: 0.0,
                    held: [None; 8],
                    #[cfg(feature = "audio")]
                    audio_queue,
                    #[cfg(feature = "audio")]
                    audio_rate,
                    #[cfg(feature = "audio")]
                    _audio: audio.stream,
                    #[cfg(not(feature = "audio"))]
                    _audio: audio,
                });
                self.screen = Screen::Playing;
                self.overlay = Overlay::None;
                self.fullscreen = false;
                self.message = None;
                // A stream named after the game it carries must restart under
                // the new name rather than appending a second game to the
                // first one's file. A user-chosen path is left alone.
                if self.video.is_enabled() && self.video_named {
                    self.video.flush();
                    self.video = VideoOut::disabled();
                }
                if self.video_autostart {
                    self.start_video();
                }
                // Config problems follow you into the game — silently ignoring
                // a key the user set is the bug being fixed here.
                if let Some(w) = self.config_warnings.first().cloned() {
                    self.set_toast(format!("⚠ {w}"));
                }
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
            if let Screen::Playing = self.screen {
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
                terminal.draw(|f| self.render(f))?;
                // The picture goes out *after* the text frame has been
                // flushed, so it is never clipped, re-diffed or overwritten by
                // `ratatui`. Nothing is written when the frame was unchanged.
                self.flush_image()?;
                let last_draw_cost = t.elapsed();
                // Never faster than the panel refresh, never slower than the
                // draw itself can sustain.
                next_draw = next_draw_at(t, last_draw_cost);
                dirty = false;
                self.emit_telemetry(last_draw_cost);
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
            s.gb.frame();
            s.t_emulate = telemetry::ms(t.elapsed());
            s.emu_count += 1;
            s.emu_frames += 1;
            s.skipped_since_display += 1;
            let elapsed = s.fps_mark.elapsed();
            if elapsed >= Duration::from_secs(1) {
                let secs = elapsed.as_secs_f32();
                s.fps = s.disp_count as f32 / secs;
                s.emu_fps = s.emu_count as f32 / secs;
                s.disp_count = 0;
                s.emu_count = 0;
                s.fps_mark = Instant::now();
            }
        }
        // Feed the video stream from the **emulated** frame, not the displayed
        // one: display frames are deliberately dropped on a large window
        // (see `crate::termimage`) and a video stream may not be.
        self.video_tick(true);
    }

    /// Start or stop the raw video stream while the game runs (`v`).
    ///
    /// The captain's rule: *never be unsure whether you are live*. So the
    /// Status panel carries a `Video` row, the toggle raises a toast, and the
    /// full `ffmpeg` command lands in the `?` overlay's notices.
    ///
    /// With a `video_out` destination configured this simply arms and disarms
    /// it. With none, it records to a file named after the game in
    /// [`video_dir`] — that is what "start streaming when I decide to" means
    /// when no consumer is waiting on a FIFO.
    fn toggle_video(&mut self) {
        if self.video.is_enabled() {
            let s = self.video.stats();
            self.video.flush();
            self.video = VideoOut::disabled();
            self.video_autostart = false;
            self.set_toast(format!(
                "Video stream stopped — {} frames, {:.1} MB",
                s.frames,
                s.bytes as f64 / 1_048_576.0
            ));
            return;
        }
        self.start_video();
        self.video_autostart = self.video.is_enabled();
    }

    fn start_video(&mut self) {
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
            // Nothing configured: record to a file named after the game.
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
        let short = match &spec.dest {
            video::Destination::Stdout => "stdout".to_string(),
            video::Destination::Path(p) => p.display().to_string(),
        };
        let (v, err) = VideoOut::open(spec);
        match err {
            Some(e) => {
                // The V4L2 refusal is several lines of instructions; the toast
                // gets the headline and the `?` overlay gets all of it.
                self.set_toast(format!(
                    "Video failed: {}",
                    e.lines().next().unwrap_or("see ? for details")
                ));
                self.config_warnings.push(e);
                return;
            }
            None => {
                self.set_toast(format!("● LIVE → {short}"));
                self.config_warnings.push(v.describe());
                self.video = v;
                self.video_named = named;
            }
        }
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
    fn video_status(&self) -> String {
        if !self.video.is_enabled() {
            return "off".to_string();
        }
        let s = self.video.stats();
        format!("● LIVE {:.0}M", s.bytes as f64 / 1_048_576.0)
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
    fn flush_image(&mut self) -> io::Result<()> {
        let Some(s) = self.session.as_mut() else {
            return Ok(());
        };
        if s.pending_wire.is_empty() {
            return Ok(());
        }
        use std::io::Write as _;
        let mut out = io::stdout().lock();
        out.write_all(s.pending_wire.as_bytes())?;
        out.flush()?;
        s.pending_wire.clear();
        Ok(())
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
        };
        self.telemetry.frame(&ctx, &sample);
    }

    fn release_expired(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        if let Some(s) = self.session.as_mut() {
            for idx in 0..8 {
                if let Some(t) = s.held[idx] {
                    if now.duration_since(t) >= KEY_HOLD {
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
        let press = key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat;
        let release = key.kind == KeyEventKind::Release;

        // Control keys act on press only.
        if press {
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
                // nobody has to look up.
                KeyCode::Char('v') => {
                    self.toggle_video();
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
                // Speed control: `+`/`=` faster, `-`/`_` slower.
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.bump_speed(true);
                    return;
                }
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    self.bump_speed(false);
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

        // Game-button keys (movement + configurable A/B/Start/Select).
        if let Some(button) = self.map_button(&key.code) {
            if press {
                self.press_button(button);
            } else if release {
                self.release_button(button);
            }
        }
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

    fn press_button(&mut self, button: KeypadKey) {
        if let Some(s) = self.session.as_mut() {
            s.gb.keydown(button);
            s.held[key_index(button)] = Some(Instant::now());
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

        let header = format!(" gameboy · pick a ROM — {}", self.config.rom_dir.display());
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
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(0),    // body
                Constraint::Length(1), // footer
            ])
            .split(area);

        let title = self
            .session
            .as_ref()
            .map(|s| s.entry.title.clone())
            .unwrap_or_default();
        let paused = self.session.as_ref().map(|s| s.paused).unwrap_or(false);
        let header = if paused {
            format!(" gameboy · {title}  [PAUSED]")
        } else {
            format!(" gameboy · {title}")
        };
        f.render_widget(
            Paragraph::new(header).style(Style::default().add_modifier(Modifier::BOLD)),
            rows[0],
        );

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(30), Constraint::Length(30)])
            .split(rows[1]);

        // The stats panel lives in the sidebar, above Status — never over the
        // picture.
        let game = body[0];
        if self.stats {
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

        // The toast replaces the footer hint while it is up, for the same
        // reason: it must not land on the picture.
        let footer = match self.toast_text() {
            Some(msg) => Line::from(Span::styled(
                format!(" {msg}"),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            None => Line::from(Span::styled(
                " ?help · p pause · +/- speed · t stats · F5/F8 save/load · f full · q quit",
                Style::default().fg(Color::DarkGray),
            )),
        };
        f.render_widget(Paragraph::new(footer), rows[2]);
    }

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
            Line::from(format!("spd  {speed_pct:.0}% of real")),
            Line::from(format!("emu  {:.2}ms", s.t_emulate)),
            Line::from(format!("pal  {:.2}ms", s.t_palette)),
            Line::from(format!("up   {:.2}ms", s.t_upscale)),
            Line::from(format!("enc  {:.2}ms", s.t_encode)),
            Line::from(format!("tx   {:.0}K", s.tx_bytes as f32 / 1024.0)),
            Line::from(format!("drop {}", s.frames_dropped)),
            Line::from(format!("same {}", s.frames_identical)),
            Line::from(format!("pc   {:04X}", s.gb.debug_pc())),
            Line::from(format!("bank {}", s.gb.debug_rom_bank())),
        ];
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
        Some(TermImage::new(wire, 0x0067_6201))
    }

    /// The upscale factor actually applied before transmission (see
    /// [`crate::screen::config::TransmitScale`]) — usually smaller than the
    /// factor shown in the status
    /// panel, because the terminal does the rest of the enlargement for free.
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
        let sig = (
            match self.overlay {
                Overlay::None => 0u8,
                Overlay::Help => 1,
                Overlay::Quit => 2,
            },
            self.fullscreen,
            self.stats,
            self.toast_text().is_some(),
        );
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
                s.tx_bytes = 0;
                if let Some(proto) = s.image.as_mut() {
                    let image = StatefulImage::default().resize(Resize::Scale(None));
                    f.render_stateful_widget(image, rect, proto);
                }
            }
        }

        // Displayed-frame accounting.
        s.disp_count += 1;
        s.disp_frames += 1;
        s.frames_dropped += s.skipped_since_display.saturating_sub(1);
        s.skipped_since_display = 0;
    }

    fn render_sidebar(&mut self, f: &mut Frame<'_>, area: Rect) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(13), Constraint::Min(0)])
            .split(area);

        // Status. `Sent` is the size actually transmitted to the terminal each
        // frame, which the terminal then stretches to `Scale`; keeping those
        // two apart is what holds the frame rate up on a large window.
        let (title, mbc, scale, fps, emu_fps, paused, sent, kb) = self
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
                    s.tx_bytes as f32 / 1024.0,
                )
            })
            .unwrap_or((String::new(), "", 1.0, 0.0, 0.0, false, (160, 144), 0.0));
        let status = vec![
            kv("ROM", &truncate(&title, 18)),
            kv("MBC", mbc),
            kv("Scale", &format!("{scale:.1}x")),
            kv("Sent", &format!("{}x{} {:.0}K", sent.0, sent.1, kb)),
            kv("Proto", protocol_name(self.picker.protocol_type())),
            kv("Palette", self.palette.name()),
            kv("Upscale", &self.upscale_label()),
            kv("Speed", SPEED_LADDER[self.speed].label),
            kv("Slot", &self.slot.to_string()),
            // "Am I live?" must never be a guess — see `App::toggle_video`.
            kv("Video", &self.video_status()),
            kv(
                "FPS",
                &if paused {
                    "paused".to_string()
                } else {
                    format!("{fps:.0} · emu {emu_fps:.0}")
                },
            ),
        ];
        f.render_widget(
            Paragraph::new(status)
                .block(Block::default().borders(Borders::ALL).title(" Status ")),
            split[0],
        );

        // Controls.
        let k = &self.config.keys;
        let controls = vec![
            Line::from("Arrows  move"),
            Line::from(format!("{}       A", k.a)),
            Line::from(format!("{}       B", k.b)),
            Line::from(format!("{}       Start", k.start)),
            Line::from(format!("{}       Select", k.select)),
            Line::from(""),
            Line::from("p pause     R reset"),
            Line::from("+/- speed   t stats"),
            Line::from("1-3 slot"),
            Line::from("F5 save     F8 load"),
            Line::from("L load ROM  f full"),
            Line::from("[ palette   ? help"),
            Line::from("] upscale   } factor"),
            Line::from("c shot      C raw shot"),
            Line::from("v video out"),
        ];
        f.render_widget(
            Paragraph::new(controls)
                .block(Block::default().borders(Borders::ALL).title(" Controls ")),
            split[1],
        );
    }

    fn render_help(&self, f: &mut Frame<'_>) {
        let area = popup_area(f.area(), 60, 60);
        f.render_widget(Clear, area);
        let text = vec![
            Line::from(Span::styled(
                "Help",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Picker"),
            Line::from("  ↑/↓ j/k   move        enter   play"),
            Line::from("  /         filter      r       rescan"),
            Line::from("  .         resume last g/G     top/bottom"),
            Line::from("  q/esc     quit"),
            Line::from(""),
            Line::from("Game"),
            Line::from("  arrows    move        a/s     A / B"),
            Line::from("  x/z       Start/Sel   p       pause"),
            Line::from("  +/-       speed        t       stats overlay"),
            Line::from("  1-3       save slot    F5/F8   save / load"),
            Line::from("  R         reset       L       load ROM"),
            Line::from("  f         fullscreen  [       palette"),
            Line::from("  ]         upscaler    }       upscale factor"),
            Line::from("  c         screenshot  C       screenshot (raw 160x144)"),
            Line::from("  v         video stream on/off (see docs/video-out.md)"),
            Line::from("  ?         this help   q       quit"),
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
        ];
        // A config key that did not apply is repeated here, so it cannot be
        // missed by someone who started the emulator from a launcher and never
        // saw the startup stderr.
        let mut text = text;
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
            Line::from("Quit gameboy?"),
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

fn kv<'a>(key: &'a str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<8}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_string()),
    ])
}

/// Build a [`Picker`], preferring the sharpest graphics protocol the terminal
/// actually advertises (kept from the dead-code/audio crew).
///
/// `Picker::from_query_stdio()` lets an env-var guess (e.g. `TERM_PROGRAM=WezTerm`)
/// force the fuzzy iTerm2 protocol even when the terminal advertises the crisper
/// Kitty graphics protocol. We first query with those env guesses suppressed so
/// the *advertised* protocol wins; only if the terminal advertises no real
/// graphics protocol do we fall back to the env-based guess. Never panics on a
/// query timeout — falls back to a sane default font size instead.
fn build_picker() -> Picker {
    let saved_term_program = env::var_os("TERM_PROGRAM");
    let saved_lc_terminal = env::var_os("LC_TERMINAL");
    env::remove_var("TERM_PROGRAM");
    env::remove_var("LC_TERMINAL");
    let advertised = Picker::from_query_stdio();
    if let Some(v) = saved_term_program {
        env::set_var("TERM_PROGRAM", v);
    }
    if let Some(v) = saved_lc_terminal {
        env::set_var("LC_TERMINAL", v);
    }

    match advertised {
        // The terminal advertised a real graphics protocol — use it.
        Ok(p) if p.protocol_type() != ProtocolType::Halfblocks => p,
        // Otherwise fall back to the env-guess query (e.g. a genuine iTerm2), and
        // if even that fails (timeout / no capabilities) use a default rather
        // than panicking.
        _ => {
            Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((10, 20)))
        }
    }
}

/// Resolve a [`CellScale`] to a concrete device-pixels-per-cell multiplier.
///
/// Priority (first hit wins):
///   1. `$GAMEBOY_CELL_SCALE` — a quick override for testing, beats the config.
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
    if let Some(v) = env::var("GAMEBOY_CELL_SCALE")
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

/// Build a [`Gameboy`], its [`RomEntry`] and its audio hold from a ROM on disk.
///
/// NOTE: always pass `Some(path)` to `Gameboy::new` so the MBC is detected from
/// the header — with `None`, `get_mbc` forces MBC3 and mis-drives other carts.
// `audio` is `()` without the audio feature; that unit binding is intentional.
#[cfg_attr(not(feature = "audio"), allow(clippy::let_unit_value))]
fn load_session(path: &Path) -> Result<(Gameboy, RomEntry, AudioHold), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if data.len() < 0x150 {
        return Err("file is too small to be a Game Boy ROM".to_string());
    }
    let entry = romlib::entry_from_bytes(path, &data);
    let mut gb = Gameboy::new(data, Some(path.to_path_buf()));
    let audio = init_audio(&mut gb);
    Ok((gb, entry, audio))
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
    fn test_app(protocol: ProtocolType, path: RenderPath) -> Option<App> {
        test_app_video(protocol, path, video_for_test())
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
        // 160x144 image whatever the window size, plus kitty's own per-cell
        // placeholder grid.
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        let small = drawn_bytes(&mut app, 80, 24);
        app.session.as_mut().unwrap().gb.frame();
        let large = drawn_bytes(&mut app, 320, 90);
        assert!(small > 1000, "expected a real payload, got {small} bytes");
        assert!(
            large < 300_000,
            "a 1568x1408 display cost {large} bytes/frame (legacy: 11.6 MB)"
        );
    }

    #[test]
    fn kitty_placement_asks_the_terminal_to_scale() {
        let mut app = test_app(ProtocolType::Kitty, RenderPath::Fast).unwrap();
        let wire = drawn_wire(&mut app, 160, 50);
        assert!(
            wire.contains("a=T,U=1"),
            "no kitty virtual placement emitted"
        );
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
                        _ => "a=T,U=1",
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

    /// The `v` key must arm and disarm the stream mid-game, name the file
    /// after the cartridge, and leave no doubt about whether it is live.
    #[test]
    fn the_v_key_toggles_the_stream_and_names_it_after_the_game() {
        let dir = std::env::temp_dir()
            .join(format!("gameboy-vid-test/{}-toggle", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Some(mut app) = test_app(ProtocolType::Kitty, RenderPath::Fast) else {
            return;
        };
        app.config.video_dir = Some(dir.to_string_lossy().into_owned());
        assert_eq!(app.video_status(), "off");

        // On.
        app.toggle_video();
        assert!(app.video.is_enabled(), "toast: {:?}", app.toast);
        let toast = app.toast.as_ref().map(|(t, _)| t.clone()).unwrap();
        assert!(toast.starts_with("● LIVE → "), "{toast}");
        assert!(app.video_status().starts_with("● LIVE"));

        for _ in 0..5 {
            app.step_frame();
        }

        // Off.
        app.toggle_video();
        assert!(!app.video.is_enabled());
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

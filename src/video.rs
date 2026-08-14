//! Video-out — the emulator's picture as a byte stream other tools consume.
//!
//! # Why
//!
//! [`crate::telemetry`] is the *numbers* half of the emit surface designed in
//! [`docs/debugging.md`](../docs/debugging.md): the "frame rendered" event,
//! published as NDJSON so a program can answer "did that change help?" without
//! a human watching. This module is the **pixels** half of the same event: the
//! frame buffer itself, published as a raw video stream, so a program can
//! *watch* the emulator without being a terminal.
//!
//! Deliberately **not** a streaming server. It writes frames; `ffmpeg` already
//! does RTMP, recording, scaling, muxing and v4l2 output far better than we
//! ever would. Our job is to produce frames cleanly and to document exactly how
//! to consume them.
//!
//! # Turning it on
//!
//! Off by default. When off it costs one `Option` test per emulated frame.
//!
//! | Setting | Env | Config key | Values |
//! |---|---|---|---|
//! | destination | `GB_VIDEO_OUT` | `video_out` | `-`/`stdout`, a file, a FIFO, `off` |
//! | pixel format | `GB_VIDEO_FORMAT` | `video_format` | `rgba` (default), `rgb24`, `yuyv` |
//! | frame size | `GB_VIDEO_SIZE` | `video_size` | `WxH`, e.g. `1280x720` |
//! | picture source | `GB_VIDEO_SOURCE` | `video_source` | `native` (default), `display` |
//! | pacing | `GB_VIDEO_PACE` | `video_pace` | `realtime` (default), `emulated` |
//!
//! The environment wins over the config file, as with telemetry.
//!
//! **Every one of these is safe to set.** A destination that cannot be written
//! to right now — most obviously a FIFO with no consumer on the other end —
//! parks the *stream* in [`StreamState::Waiting`] and never the emulator. See
//! "A FIFO needs a consumer" below.
//!
//! # Two intents: recording, and feeding something live
//!
//! Writing a file you keep and feeding a live consumer are different jobs, and
//! [`Intent`] names which one is happening so a frontend can say so rather than
//! showing one ambiguous `LIVE`:
//!
//! * [`Intent::Record`] — a regular file. Every byte is kept, nothing is
//!   dropped for being late, and the file grows until the stream stops.
//! * [`Intent::Stream`] — stdout, or a **FIFO**. Something is reading at the far
//!   end, so the stream has the two states a consumer implies (`Waiting`,
//!   `Live`) and frames are dropped rather than made to wait.
//!
//! The intent is *derived from the destination*, never guessed: a path that
//! `stat`s as a named pipe is a stream, anything else is a recording.
//!
//! # A FIFO needs a consumer — and must never hold up the emulator
//!
//! `open(2)` on a FIFO for writing **blocks until a reader attaches**. Opening
//! one during startup therefore hung the whole emulator before any UI existed,
//! with nothing the user could do but kill it. That is fixed three ways, and
//! all three matter:
//!
//! 1. **The open is non-blocking** (`O_NONBLOCK`). "No reader yet" comes back
//!    as `ENXIO`, which is a *state* ([`StreamState::Waiting`]), not a wait.
//! 2. **The open is retried** every [`RETRY_EVERY`] while the stream is armed,
//!    so starting the emulator *before* the consumer works exactly as well as
//!    the other order. A consumer that goes away (`EPIPE`) drops the stream
//!    back to `Waiting`, where the next one is picked up the same way.
//! 3. **The writes are non-blocking too.** A stalled or slow consumer fills the
//!    pipe buffer; `write(2)` then returns `EAGAIN` and the frame is
//!    **dropped** ([`VideoStats::stalled`]). Emulation is never charged for a
//!    consumer's speed. `rawvideo` is positional, so a frame written only in
//!    part is finished before the next one starts — the consumer sees whole
//!    frames or nothing.
//!
//! `SIGPIPE` is set to `SIG_IGN` before the first byte is written, so a
//! consumer quitting delivers `EPIPE` to us instead of killing the process.
//!
//! # Consuming it
//!
//! [`VideoOut::ffmpeg_hint`] prints the exact command for the *current*
//! settings, so the answer is never guesswork:
//!
//! ```text
//! mkfifo /tmp/gb.raw
//! ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i /tmp/gb.raw -pix_fmt yuv420p gb.mp4
//! GB_VIDEO_OUT=/tmp/gb.raw cargo run --release --example terminal -- game.gb
//! ```
//!
//! …and it need not be read off a screen either: while a FIFO stream is armed
//! the emulator keeps a **sidecar descriptor** beside it, `<fifo>.gbvideo`,
//! naming the geometry, pixel format, frame rate and game. `rawvideo` carries
//! none of that in-band, so without it the consumer has to be told the same
//! numbers a second time and silently produces garbage when the two disagree —
//! which is exactly how a 160×144 `rgba` stream ended up pointed at a bridge
//! expecting 640×480 `yuyv`. `tools/virtual-camera.sh` reads it.
//!
//! # Geometry is fixed for the life of the stream
//!
//! `rawvideo` carries no header: the consumer is told `-s WxH` once and every
//! byte after that is positional. So the output geometry is decided when the
//! first frame arrives and never changes. A frame of a different size (the user
//! cycled the upscaler mid-stream) is **letterboxed** into the fixed geometry
//! rather than corrupting the stream.
//!
//! # Pacing
//!
//! `rawvideo` has no timestamps either — frame *number* is the clock. So the
//! stream must carry one frame per 1/59.7275 s of **wall clock**, whatever the
//! emulator is doing:
//!
//! * `realtime` (default) — fast-forward drops frames, slow motion and pause
//!   repeat the last one, so a 60-second recording is 60 seconds long and the
//!   game in it moves at the speed the player saw. This is what OBS and a
//!   webcam want.
//! * `emulated` — exactly one output frame per emulated frame, no duplication
//!   and no dropping. A fast-forwarded capture plays back fast; a headless run
//!   produces a deterministic file as fast as the CPU allows. This is the right
//!   mode for offline capture (`examples/video_out.rs`).
//!
//! # Platform
//!
//! The stream itself is portable (a pipe is a pipe). The **virtual camera** on
//! the far end is not: `v4l2loopback` is Linux-only. macOS needs a CoreMedia IO
//! extension (or OBS's built-in virtual camera), Windows a DirectShow filter —
//! both out of scope here, and both reachable the same way, by pointing
//! `ffmpeg`/OBS at this stream. See [`docs/video-out.md`](../docs/video-out.md).

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Native Game Boy picture size.
pub const SRC_W: u32 = 160;
pub const SRC_H: u32 = 144;

/// True DMG refresh rate, and the wall-clock period of one frame.
pub const DMG_HZ: f64 = 59.7275;
pub const FRAME_PERIOD: Duration = Duration::from_micros(16_743);

/// A frame arriving this far *before* its deadline still counts as due.
/// Without it, ordinary scheduling jitter turns a steady 1:1 stream into an
/// occasional "drop one, duplicate one" stutter even at exactly 1× speed.
const SLACK: Duration = Duration::from_millis(2);

/// Most duplicate frames emitted from one pump. Bounds the burst after a stall
/// (suspend, a long modal pause) instead of dumping seconds of video at once.
const MAX_BURST: u32 = 4;

/// How often the idle card is re-sent while the game feed is switched off.
///
/// **Idle must be a trickle, not silence.** A camera that stops producing is a
/// camera consumers start treating as broken — and the whole point of holding
/// the device is that a viewer mid-call notices nothing but the picture
/// changing. Two frames a second is far above anything a consumer times out on
/// and 30× less work than the game feed; the card is converted once and cached,
/// so a tick is one `write(2)` of an unchanged buffer.
pub const IDLE_PERIOD: Duration = Duration::from_millis(500);

/// How often an armed FIFO retries its `open` while no consumer is attached.
///
/// A failed non-blocking open is one cheap syscall, but doing it 60 times a
/// second for an hour is noise in `strace` for no gain; a quarter of a second
/// is far below what anyone notices when they start the bridge by hand.
pub const RETRY_EVERY: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Where frames go, before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Stdout,
    Path(PathBuf),
}

impl Destination {
    /// Parse a `GB_VIDEO_OUT` / `video_out = …` value. Empty, `off`, `0` and
    /// `false` mean disabled.
    pub fn parse(value: &str) -> Option<Destination> {
        let v = value.trim();
        if v.is_empty()
            || v.eq_ignore_ascii_case("off")
            || v.eq_ignore_ascii_case("false")
            || v == "0"
        {
            return None;
        }
        if v == "-" || v.eq_ignore_ascii_case("stdout") {
            return Some(Destination::Stdout);
        }
        Some(Destination::Path(PathBuf::from(v)))
    }

    /// How a consumer names this destination on an `ffmpeg` command line.
    fn ffmpeg_input(&self) -> String {
        match self {
            Destination::Stdout => "-".to_string(),
            Destination::Path(p) => p.display().to_string(),
        }
    }

    /// Short form for a status row: the path, or `stdout`.
    pub fn label(&self) -> String {
        self.ffmpeg_input()
    }

    /// Is this a live consumer or a file being kept? Derived from what the
    /// destination *is* — see the module docs on the two intents.
    pub fn intent(&self) -> Intent {
        match self {
            Destination::Stdout => Intent::Stream,
            Destination::Path(p) if is_fifo(p) || v4l2::is_video_device(p) => {
                Intent::Stream
            }
            Destination::Path(_) => Intent::Record,
        }
    }
}

/// What the user is actually doing with the stream. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// A live consumer at the far end (a FIFO, or stdout in a pipe).
    Stream,
    /// A file being written and kept.
    Record,
}

impl Intent {
    /// The word a frontend puts in front of the destination. Deliberately not
    /// interchangeable: `REC` cannot be mistaken for a camera feed, and `LIVE`
    /// cannot be mistaken for a file on disk.
    pub fn word(self) -> &'static str {
        match self {
            Intent::Stream => "LIVE",
            Intent::Record => "REC",
        }
    }
}

/// Whether frames are reaching anything, right now.
///
/// The middle state is the whole point of this enum: an armed FIFO with no
/// consumer is neither off nor live, and calling it either one is what made a
/// misconfigured camera bridge look like a working one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// Nothing configured, or the stream was stopped.
    Off,
    /// Armed, but nothing is reading yet — the `open` is retried every
    /// [`RETRY_EVERY`], so this becomes `Live` the moment a consumer appears.
    Waiting,
    /// Frames are being written.
    Live,
}

impl StreamState {
    pub fn name(self) -> &'static str {
        match self {
            StreamState::Off => "off",
            StreamState::Waiting => "waiting",
            StreamState::Live => "live",
        }
    }
}

/// Is this path a named pipe?
///
/// Load-bearing: it is what tells a *stream* from a *recording*, and what
/// selects the non-blocking open. A path that does not exist yet is not a
/// FIFO — it will be created as a regular file, i.e. a recording.
#[cfg(unix)]
pub fn is_fifo(p: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(p)
        .map(|m| m.file_type().is_fifo())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn is_fifo(_p: &Path) -> bool {
    false
}

/// Wire pixel format. All are uncompressed and fixed-size, so a consumer needs
/// only `-pix_fmt` and `-s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4 bytes/pixel, exactly the emulator's framebuffer — zero conversion.
    Rgba,
    /// 3 bytes/pixel. 25 % fewer bytes; what most encoders want.
    Rgb24,
    /// Packed 4:2:2 `Y0 Cb Y1 Cr`, BT.601 limited range — the format real
    /// webcams emit and the one `v4l2loopback` consumers accept most widely.
    Yuyv422,
}

impl PixelFormat {
    pub fn parse(v: &str) -> Option<PixelFormat> {
        match v.trim().to_ascii_lowercase().as_str() {
            "rgba" | "rgb32" => Some(PixelFormat::Rgba),
            "rgb24" | "rgb" => Some(PixelFormat::Rgb24),
            "yuyv" | "yuyv422" | "yuy2" => Some(PixelFormat::Yuyv422),
            _ => None,
        }
    }

    /// The `-pix_fmt` name ffmpeg knows it by (identical to ours by design).
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            PixelFormat::Rgba => "rgba",
            PixelFormat::Rgb24 => "rgb24",
            PixelFormat::Yuyv422 => "yuyv422",
        }
    }

    pub fn bytes_per_frame(self, w: u32, h: u32) -> usize {
        let px = w as usize * h as usize;
        match self {
            PixelFormat::Rgba => px * 4,
            PixelFormat::Rgb24 => px * 3,
            PixelFormat::Yuyv422 => px * 2,
        }
    }

    /// 4:2:2 packs two pixels per macropixel, so the width must be even.
    fn needs_even_width(self) -> bool {
        self == PixelFormat::Yuyv422
    }
}

/// Which picture is streamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The raw 160×144 framebuffer: the honest source, letting the consumer
    /// scale and colour it however it likes.
    Native,
    /// What the frontend shows — palette-mapped and run through the pixel-art
    /// upscaler, so the stream carries the LCD/MMPX look.
    Display,
}

impl Source {
    pub fn parse(v: &str) -> Option<Source> {
        match v.trim().to_ascii_lowercase().as_str() {
            "native" | "raw" => Some(Source::Native),
            "display" | "screen" => Some(Source::Display),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Native => "native",
            Source::Display => "display",
        }
    }
}

/// How output frames are paced. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Realtime,
    Emulated,
}

impl Pace {
    pub fn parse(v: &str) -> Option<Pace> {
        match v.trim().to_ascii_lowercase().as_str() {
            "realtime" | "real" | "wall" => Some(Pace::Realtime),
            "emulated" | "emu" | "asap" => Some(Pace::Emulated),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Pace::Realtime => "realtime",
            Pace::Emulated => "emulated",
        }
    }
}

/// The raw strings a frontend collected from its config file, before parsing.
/// Keeping the *parsing* here means the config file and the environment cannot
/// drift apart, and every complaint is worded once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawSettings {
    pub out: Option<String>,
    pub format: Option<String>,
    pub size: Option<String>,
    pub source: Option<String>,
    pub pace: Option<String>,
    /// Should the stream be **live from launch**? See [`resolve_autostart`].
    pub autostart: Option<String>,
}

/// A fully-resolved stream configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSpec {
    pub dest: Destination,
    pub format: PixelFormat,
    pub source: Source,
    /// Fixed output geometry. `None` means "whatever the first frame is".
    pub size: Option<(u32, u32)>,
    pub pace: Pace,
    /// The cartridge title, when the frontend knows it. Carried only so the
    /// sidecar descriptor can name the game — which is how the virtual camera
    /// ends up labelled `POKEMON BLUE` instead of `Dummy video device`.
    pub title: Option<String>,
}

impl VideoSpec {
    /// Defaults for a destination: native 160×144 RGBA at wall-clock rate.
    pub fn new(dest: Destination) -> VideoSpec {
        VideoSpec {
            dest,
            format: PixelFormat::Rgba,
            source: Source::Native,
            size: None,
            pace: Pace::Realtime,
            title: None,
        }
    }

    /// Resolve config strings, then let the environment override each field.
    /// Returns `None` when video-out is not enabled, plus a complaint for every
    /// value that could not be applied (same contract as config parsing: a
    /// mistyped key must never fail silently).
    pub fn resolve(raw: &RawSettings) -> (Option<VideoSpec>, Vec<String>) {
        let mut warn = Vec::new();
        let pick = |key: &str, cfg: &Option<String>| -> Option<String> {
            std::env::var(key)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| cfg.clone())
        };

        let Some(out) = pick("GB_VIDEO_OUT", &raw.out) else {
            return (None, warn);
        };
        let Some(dest) = Destination::parse(&out) else {
            return (None, warn);
        };
        let mut spec = VideoSpec::new(dest);

        if let Some(v) = pick("GB_VIDEO_FORMAT", &raw.format) {
            match PixelFormat::parse(&v) {
                Some(f) => spec.format = f,
                None => warn.push(bad("video_format", &v, "rgba, rgb24, yuyv")),
            }
        }
        if let Some(v) = pick("GB_VIDEO_SOURCE", &raw.source) {
            match Source::parse(&v) {
                Some(s) => spec.source = s,
                None => warn.push(bad("video_source", &v, "native, display")),
            }
        }
        if let Some(v) = pick("GB_VIDEO_PACE", &raw.pace) {
            match Pace::parse(&v) {
                Some(p) => spec.pace = p,
                None => warn.push(bad("video_pace", &v, "realtime, emulated")),
            }
        }
        if let Some(v) = pick("GB_VIDEO_SIZE", &raw.size) {
            match parse_size(&v) {
                Some((w, h)) => spec.size = Some(spec.even_width(w, h, &mut warn)),
                None => warn.push(bad("video_size", &v, "WxH, e.g. 640x480")),
            }
        }
        (Some(spec), warn)
    }

    /// The exact command a consumer runs against this stream. `geometry`
    /// overrides [`VideoSpec::size`] once the first frame has fixed it.
    pub fn ffmpeg_hint(&self, geometry: Option<(u32, u32)>) -> String {
        let (w, h) = geometry.or(self.size).unwrap_or((SRC_W, SRC_H));
        format!(
            "ffmpeg -f rawvideo -pix_fmt {} -s {w}x{h} -framerate {DMG_HZ} -i {} \
             -pix_fmt yuv420p out.mp4",
            self.format.ffmpeg_name(),
            self.dest.ffmpeg_input(),
        )
    }

    /// One line describing the stream, for a frontend to show the user.
    ///
    /// A camera gets different words: there is no command to consume it with,
    /// and telling someone to run `ffmpeg` at the thing already appearing in
    /// their camera picker is how the bridge got mistaken for the product.
    pub fn describe(&self, geometry: Option<(u32, u32)>) -> String {
        let (w, h) = geometry.or(self.size).unwrap_or((SRC_W, SRC_H));
        if let Destination::Path(p) = &self.dest {
            if v4l2::is_video_device(p) && v4l2::NATIVE_OUTPUT {
                let label = v4l2::device_label(p).unwrap_or_default();
                return format!(
                    "video: {w}x{h} yuyv422 -> {} — pick {} in any camera picker \
                     (Chrome, Meet, Discord, OBS)",
                    p.display(),
                    if label.is_empty() {
                        "the camera".to_string()
                    } else {
                        format!("\"{label}\"")
                    },
                );
            }
        }
        format!(
            "video: {w}x{h} {} ({}, {}) -> {} | consume with: {}",
            self.format.ffmpeg_name(),
            self.source.name(),
            self.pace.name(),
            self.dest.ffmpeg_input(),
            self.ffmpeg_hint(geometry),
        )
    }

    /// 4:2:2 cannot express an odd width; round up and say so rather than
    /// emitting a stream every consumer mis-parses by one column per row.
    fn even_width(&self, w: u32, h: u32, warn: &mut Vec<String>) -> (u32, u32) {
        if self.format.needs_even_width() && w % 2 == 1 {
            warn.push(format!(
                "video_size: {w}x{h} has an odd width, which yuyv cannot pack — using {}x{h}",
                w + 1
            ));
            (w + 1, h)
        } else {
            (w, h)
        }
    }
}

/// Does the user want the stream **live from launch**?
///
/// This is deliberately a separate question from [`VideoSpec::resolve`], because
/// they are separate decisions and conflating them was a bug: naming a
/// destination says *where* frames go, it does not say *send them*. A configured
/// `video_out` used to mean "streaming, now, before the UI exists" — so a
/// terminal launched with a camera configured came up already feeding it, and
/// (worse, earlier) a FIFO destination parked the process in `open(2)` at
/// startup with no interface and nothing to do but `kill`.
///
/// So: the default is **off**, `v` is what goes live, and wanting it live from
/// launch is its own explicit `video_autostart = true` / `--video-start`.
/// `GB_VIDEO_START` overrides the config key, as everywhere else here.
pub fn resolve_autostart(raw: &RawSettings) -> (bool, Vec<String>) {
    let mut warn = Vec::new();
    let v = std::env::var("GB_VIDEO_START")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| raw.autostart.clone());
    let Some(v) = v else {
        return (false, warn);
    };
    match parse_bool(&v) {
        Some(b) => (b, warn),
        None => {
            warn.push(bad("video_autostart", &v, "true or false"));
            (false, warn)
        }
    }
}

/// The spellings a person actually types for a yes/no config value.
fn parse_bool(v: &str) -> Option<bool> {
    let v = v.trim();
    if v.is_empty() {
        return Some(false);
    }
    for y in ["1", "true", "yes", "on"] {
        if v.eq_ignore_ascii_case(y) {
            return Some(true);
        }
    }
    for n in ["0", "false", "no", "off"] {
        if v.eq_ignore_ascii_case(n) {
            return Some(false);
        }
    }
    None
}

fn bad(key: &str, value: &str, expected: &str) -> String {
    format!("config: `{key} = {value}` is not valid (expected {expected})")
}

/// `640x480`, `640X480`, `640*480`.
pub fn parse_size(v: &str) -> Option<(u32, u32)> {
    let v = v.trim();
    let (a, b) = v
        .split_once(['x', 'X', '*'])
        .map(|(a, b)| (a.trim(), b.trim()))?;
    let w: u32 = a.parse().ok()?;
    let h: u32 = b.parse().ok()?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return None;
    }
    Some((w, h))
}

// ---------------------------------------------------------------------------
// Naming the stream after the game
// ---------------------------------------------------------------------------

/// A cartridge title as a human would write it.
///
/// The header title is 11–16 bytes of upper-case ASCII, padded with NULs or
/// spaces, and a cartridge may put anything at all in there — so control bytes
/// and non-ASCII are dropped, runs of spaces collapsed, and the result trimmed.
/// This is what goes in a device label, a stream key or a window title.
pub fn clean_title(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut space = false;
    for c in raw.chars() {
        let c = if c.is_ascii_graphic() {
            c
        } else if c == ' ' || c == '\t' || c == '\u{0}' {
            ' '
        } else {
            continue;
        };
        if c == ' ' {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// A cartridge title as a filename component: ASCII alphanumerics kept, every
/// thing else an underscore, no leading/trailing underscores. Never empty, and
/// never anything a path could interpret (`/`, `..`).
pub fn file_safe_title(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = safe.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "rom".to_string()
    } else {
        trimmed
    }
}

/// The filename a recording of `title` gets: the game, when it was taken, and
/// **the two things `rawvideo` cannot carry in-band** — geometry and pixel
/// format — so the consuming command can be read straight off the name.
///
/// `POKEMON_BLUE-20260813-142530-160x144-rgba.raw`
pub fn recording_name(
    title: &str,
    stamp: &str,
    w: u32,
    h: u32,
    f: PixelFormat,
) -> String {
    format!(
        "{}-{stamp}-{w}x{h}-{}.raw",
        file_safe_title(title),
        f.ffmpeg_name()
    )
}

/// Expand `%t` (file-safe game title) and `%s` (timestamp) in a destination
/// path, so a config file can say `video_out = "~/Videos/%t-%s.raw"`.
pub fn expand_placeholders(path: &str, title: &str, stamp: &str) -> String {
    path.replace("%t", &file_safe_title(title))
        .replace("%s", stamp)
}

// ---------------------------------------------------------------------------
// The sidecar descriptor
// ---------------------------------------------------------------------------

/// Suffix of the descriptor written beside a FIFO: `/tmp/gb.raw.gbvideo`.
pub const DESCRIPTOR_SUFFIX: &str = ".gbvideo";

/// Where the descriptor for `fifo` lives.
pub fn descriptor_path(fifo: &Path) -> PathBuf {
    let mut s = fifo.as_os_str().to_os_string();
    s.push(DESCRIPTOR_SUFFIX);
    PathBuf::from(s)
}

/// The descriptor's contents: what `rawvideo` cannot say about itself.
///
/// The problem this solves is concrete. A raw stream is a headerless run of
/// bytes; geometry, pixel format and frame rate live *only* in the consumer's
/// command line. So the same three numbers are configured twice, in two
/// different programs, and when they drift the consumer does not fail — it
/// happily decodes garbage. That is how a 160×144 `rgba` stream came to be fed
/// to a bridge expecting 640×480 `yuyv`, with the emulator's own banner saying
/// everything was fine.
///
/// Deliberately a flat `key=value` file with no quoting and no nesting: the
/// consumer is a shell script, and anything richer would need a parser on the
/// far end. Same reasoning as the frontend's config parser.
///
/// It describes the **shape** of the stream, not its liveness — a bridge that
/// starts before the emulator can read last session's descriptor and be right,
/// which is exactly what makes the start-in-either-order promise work.
pub fn descriptor(spec: &VideoSpec, geometry: Option<(u32, u32)>) -> String {
    let (w, h) = geometry.or(spec.size).unwrap_or((SRC_W, SRC_H));
    let title = spec.title.as_deref().map(clean_title).unwrap_or_default();
    format!(
        "# terminalgb video-out stream descriptor — written by src/video.rs\n\
         # `rawvideo` carries no header, so these are the numbers a consumer\n\
         # must be given. Read by tools/virtual-camera.sh.\n\
         path={}\n\
         width={w}\n\
         height={h}\n\
         format={}\n\
         fps={DMG_HZ}\n\
         title={title}\n\
         ffmpeg={}\n",
        spec.dest.ffmpeg_input(),
        spec.format.ffmpeg_name(),
        spec.ffmpeg_hint(geometry),
    )
}

// ---------------------------------------------------------------------------
// Command-line flags
// ---------------------------------------------------------------------------

/// The `--video-*` flags, and the environment variable each maps to.
const CLI_FLAGS: [(&str, &str); 5] = [
    ("--video-out", "GB_VIDEO_OUT"),
    ("--video-format", "GB_VIDEO_FORMAT"),
    ("--video-size", "GB_VIDEO_SIZE"),
    ("--video-source", "GB_VIDEO_SOURCE"),
    ("--video-pace", "GB_VIDEO_PACE"),
];

/// The `--video-*` flags that are a yes/no on their own.
///
/// `--video-start` takes no value (`--video-start=false` still works), because
/// the point of it is to be the one extra word someone types when they want the
/// stream live from launch — see [`resolve_autostart`].
const CLI_BOOL_FLAGS: [(&str, &str); 1] = [("--video-start", "GB_VIDEO_START")];

/// A `--video-*` flag found on a command line: the `GB_VIDEO_*` variable it
/// maps to, and the value given.
pub type CliFlag = (&'static str, String);

/// Split `--video-*` flags out of an argument list.
///
/// Returns `(settings, remaining args)`, or an error naming the flag that was
/// given without a value. `--video-out=-` and `--video-out -` both work.
pub fn split_cli_flags<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<(Vec<CliFlag>, Vec<String>), String> {
    let mut found = Vec::new();
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (a.clone(), None),
        };
        if let Some((_, var)) = CLI_BOOL_FLAGS.iter().find(|(f, _)| *f == name) {
            found.push((*var, inline.unwrap_or_else(|| "1".to_string())));
            continue;
        }
        match CLI_FLAGS.iter().find(|(f, _)| *f == name) {
            Some((flag, var)) => {
                let value = match inline {
                    Some(v) => v,
                    None => it.next().ok_or_else(|| format!("{flag} needs a value"))?,
                };
                found.push((*var, value));
            }
            None => rest.push(a),
        }
    }
    Ok((found, rest))
}

/// [`split_cli_flags`], applied: each flag becomes its `GB_VIDEO_*` variable,
/// so an explicit flag beats both the environment and the config file with no
/// extra precedence machinery. Returns the remaining arguments.
pub fn apply_cli_flags<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<Vec<String>, String> {
    let (found, rest) = split_cli_flags(args)?;
    for (var, value) in found {
        std::env::set_var(var, value);
    }
    Ok(rest)
}

/// One line of `--help` text listing the flags.
pub fn cli_usage() -> &'static str {
    "  --video-out <-|PATH>      where `v` sends frames (`-` = stdout, or a file/FIFO)\n  \
     --video-start             go live at launch (default: off — press `v`)\n  \
     --video-format <fmt>      rgba (default), rgb24, yuyv\n  \
     --video-size <WxH>        letterbox into this size, e.g. 640x480\n  \
     --video-source <src>      native (default) or display (palette + upscaler)\n  \
     --video-pace <pace>       realtime (default) or emulated"
}

// ---------------------------------------------------------------------------
// A FIFO that never blocks the emulator
// ---------------------------------------------------------------------------

/// Non-blocking `open`/`write` on a named pipe.
///
/// Both halves are gated on `libc`, which is a default feature and non-`wasm32`
/// only — the same shape [`crate::shmframe`] uses. Without it there is no way
/// to ask for `O_NONBLOCK`, and a *blocking* open is exactly the defect this
/// module exists to remove, so the FIFO is refused with an explanation instead.
#[cfg(all(unix, feature = "libc", not(target_arch = "wasm32")))]
mod pipe {
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    pub const SUPPORTED: bool = true;

    /// `open(path, O_WRONLY | O_NONBLOCK)`.
    ///
    /// The error that matters is **`ENXIO`**: on a FIFO it means "no reader has
    /// this open", which is the one case that is not a failure at all — it is
    /// the normal state before the consumer starts.
    pub fn open_write(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
    }

    /// Is this error "nobody is reading yet"?
    pub fn is_no_reader(e: &io::Error) -> bool {
        e.raw_os_error() == Some(libc::ENXIO)
    }

    /// Stop a consumer's exit from killing us.
    ///
    /// Rust's own runtime already does this for `bin` targets, but the FFI
    /// build is a `cdylib` loaded into somebody else's process, where it has
    /// not run. Writing to a pipe whose reader has gone is a *routine* event
    /// here — it must arrive as `EPIPE`, never as a signal.
    pub fn ignore_sigpipe() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        });
    }
}

#[cfg(not(all(unix, feature = "libc", not(target_arch = "wasm32"))))]
mod pipe {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    pub const SUPPORTED: bool = false;

    pub fn open_write(_path: &Path) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no non-blocking open on this build",
        ))
    }

    pub fn is_no_reader(_e: &io::Error) -> bool {
        false
    }

    pub fn ignore_sigpipe() {}
}

/// What one attempt to put a frame down a pipe achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Push {
    /// The whole frame is on the wire.
    Wrote,
    /// Nothing is reading; the frame went nowhere and nothing was lost.
    NoConsumer,
    /// A consumer is attached but is not keeping up — frame dropped.
    Stalled,
    /// The consumer went away mid-stream (`EPIPE`) — back to waiting.
    Disconnected,
}

/// A named pipe, held across the life of the stream.
///
/// Everything here is arranged around one rule: **the emulator's thread must
/// leave this function in bounded time, whatever the consumer is doing.**
struct Fifo {
    path: PathBuf,
    /// `None` = no consumer attached right now.
    file: Option<File>,
    /// Earliest instant the next `open` may be attempted.
    retry_at: Option<Instant>,
    /// Bytes of the frame in flight that the pipe would not take yet.
    ///
    /// `rawvideo` is positional — a consumer that receives 40 % of a frame and
    /// then the *next* frame is desynchronised for the rest of the run. So a
    /// partial write is finished before any new frame is offered, and frames
    /// that arrive while it is outstanding are dropped instead.
    tail: Vec<u8>,
    tail_at: usize,
}

impl Fifo {
    fn new(path: PathBuf) -> Fifo {
        Fifo {
            path,
            file: None,
            retry_at: None,
            tail: Vec::new(),
            tail_at: 0,
        }
    }

    fn live(&self) -> bool {
        self.file.is_some()
    }

    /// Try to attach to a consumer, at most once per [`RETRY_EVERY`].
    fn reconnect(&mut self, now: Instant) {
        if self.file.is_some() {
            return;
        }
        if let Some(t) = self.retry_at {
            if now < t {
                return;
            }
        }
        match pipe::open_write(&self.path) {
            Ok(f) => {
                self.file = Some(f);
                self.tail.clear();
                self.tail_at = 0;
                self.retry_at = None;
            }
            // `ENXIO` is the expected answer while nobody is reading; try again
            // shortly. Anything else (the FIFO deleted, permissions changed) is
            // a condition that will not clear itself four times a second, so
            // back off hard rather than burning a syscall per retry forever.
            Err(e) => {
                self.retry_at = Some(
                    now + if pipe::is_no_reader(&e) {
                        RETRY_EVERY
                    } else {
                        RETRY_EVERY * 20
                    },
                )
            }
        }
    }

    /// The consumer is gone. Drop the half-written frame with it: the next one
    /// to attach must start on a frame boundary.
    fn detach(&mut self) {
        self.file = None;
        self.tail.clear();
        self.tail_at = 0;
        self.retry_at = None; // a fresh consumer may already be waiting
    }

    /// Write as much of `buf` as the pipe will take without blocking.
    /// `Ok(n)` is how many bytes went; `Err` is a real failure (`EPIPE`).
    fn drain(f: &mut File, buf: &[u8]) -> std::io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            match f.write(&buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(n)
    }

    /// Is part of a frame still owed to the consumer?
    fn owes_tail(&self) -> bool {
        self.tail_at < self.tail.len()
    }

    /// Push what is still owed from the frame in flight, and nothing else.
    ///
    /// Called from [`VideoOut::idle`] so an outstanding tail drains whatever
    /// the pacing mode is: a consumer that unfreezes must never be left holding
    /// a partial frame just because the emulator happened to stop producing.
    fn service(&mut self) -> Push {
        let Some(f) = self.file.as_mut() else {
            return Push::NoConsumer;
        };
        match Fifo::drain(f, &self.tail[self.tail_at..]) {
            Ok(n) => self.tail_at += n,
            Err(_) => {
                self.detach();
                return Push::Disconnected;
            }
        }
        if self.owes_tail() {
            Push::Stalled
        } else {
            self.tail.clear();
            self.tail_at = 0;
            Push::Wrote
        }
    }

    /// Offer one whole frame. Never blocks, never partially advances the
    /// consumer's frame clock.
    fn push(&mut self, frame: &[u8], now: Instant) -> Push {
        self.reconnect(now);
        if self.file.is_none() {
            return Push::NoConsumer;
        }
        // Finish the frame already in flight before starting another.
        if self.owes_tail() {
            match self.service() {
                Push::Wrote => {}
                other => return other,
            }
        }
        let Some(f) = self.file.as_mut() else {
            return Push::NoConsumer;
        };
        match Fifo::drain(f, frame) {
            Ok(n) if n == frame.len() => Push::Wrote,
            Ok(n) => {
                self.tail.clear();
                self.tail.extend_from_slice(&frame[n..]);
                self.tail_at = 0;
                // The frame IS on the wire, just not all of it yet; the next
                // call finishes it. Counted as written, because it will be.
                Push::Wrote
            }
            Err(_) => {
                self.detach();
                Push::Disconnected
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------------

enum Sink {
    Stdout(std::io::Stdout),
    File(File),
    /// A named pipe: opened and written non-blocking, so a missing or slow
    /// consumer costs frames rather than the emulator's frame time.
    Fifo(Fifo),
    /// A V4L2 output node — the virtual camera, driven directly.
    #[cfg(all(target_os = "linux", feature = "libc"))]
    V4l2(v4l2::sink::V4l2Sink),
    /// In-memory, for tests.
    Memory(Vec<u8>),
}

/// Counters, published through telemetry so the stream can be checked without
/// watching it.
#[derive(Debug, Clone, Copy, Default)]
pub struct VideoStats {
    /// Frames written to the sink (including duplicates).
    pub frames: u64,
    /// Frames repeated because the emulator produced nothing in time (paused,
    /// slow motion, a stall).
    pub duplicated: u64,
    /// Emulated frames not represented in the stream (fast-forward, or the
    /// emulator running ahead of wall clock).
    pub dropped: u64,
    /// Times the pacing deadline was hopelessly late and was resynchronised
    /// rather than catching up (each one is a small jump in the video).
    pub resyncs: u64,
    pub bytes: u64,
    /// Milliseconds spent converting + writing the most recent frame.
    pub last_ms: f32,
    /// Writes that failed (a consumer that closed the pipe disables the sink).
    pub write_errors: u64,
    /// Frames thrown away because the consumer was not draining fast enough.
    /// Non-zero here is the honest symptom of "ffmpeg cannot keep up" — the
    /// emulator does not slow down to help it.
    pub stalled: u64,
    /// Times a consumer attached to the FIFO (the first is the stream going
    /// live; later ones are a bridge being restarted).
    pub attaches: u64,
    /// Times a consumer went away mid-stream.
    pub detaches: u64,
    /// Frames offered while nothing was reading. Not a fault — a stream can be
    /// armed long before its bridge starts.
    pub unconsumed: u64,
    /// Idle-card frames written while the game feed was switched off. Counted
    /// apart from `frames` on purpose: the card is the destination staying
    /// alive, not the game going out.
    pub idle_frames: u64,
}

/// The emitter. Cheap to hold; does nothing until a sink is attached.
pub struct VideoOut {
    sink: Option<Sink>,
    spec: VideoSpec,
    /// Fixed output geometry, decided by the first frame.
    out: Option<(u32, u32)>,
    /// Most recent input frame (RGBA) and its size.
    staged: Vec<u8>,
    staged_dims: (u32, u32),
    /// Scratch: the frame scaled/letterboxed to the output geometry (RGBA).
    fitted: Vec<u8>,
    /// Scratch: one output row, so repeated source rows are memcpy'd.
    row: Vec<u8>,
    /// Scratch: the wire bytes.
    wire: Vec<u8>,
    /// `wire` matches `staged`; a duplicate frame re-writes it as-is.
    wire_valid: bool,
    /// Nearest-neighbour source index per output column/row.
    xmap: Vec<u32>,
    ymap: Vec<u32>,
    /// Next wall-clock instant a frame is due (`Realtime` only).
    next: Option<Instant>,
    /// Is the *game* being sent? `false` with a sink still present is an
    /// **idling** stream — the destination is still held and still being fed,
    /// with the idle card instead of the picture. See [`VideoOut::go_idle`].
    streaming: bool,
    /// Scratch: the idle card, drawn once per geometry.
    card: Vec<u8>,
    card_dims: (u32, u32),
    /// Is `staged` currently holding the card rather than a game frame?
    card_staged: bool,
    /// Size of the last *game* frame staged, so the idle card can be drawn into
    /// the same rectangle the game occupies. See [`VideoOut::card_size`].
    game_fit: Option<(u32, u32)>,
    /// Next wall-clock instant the idle card is due.
    idle_next: Option<Instant>,
    /// Source size the `xmap`/`ymap` tables were built for. The output size
    /// alone is not enough to know they are still valid — the idle card and a
    /// `display`-source game frame are different sizes going to the same
    /// output.
    map_src: (u32, u32),
    stats: VideoStats,
}

impl Default for VideoOut {
    fn default() -> Self {
        VideoOut::disabled()
    }
}

impl VideoOut {
    /// A no-op emitter.
    pub fn disabled() -> VideoOut {
        VideoOut {
            sink: None,
            spec: VideoSpec::new(Destination::Stdout),
            out: None,
            staged: Vec::new(),
            staged_dims: (0, 0),
            fitted: Vec::new(),
            row: Vec::new(),
            wire: Vec::new(),
            wire_valid: false,
            xmap: Vec::new(),
            ymap: Vec::new(),
            next: None,
            streaming: true,
            card: Vec::new(),
            card_dims: (0, 0),
            card_staged: false,
            game_fit: None,
            idle_next: None,
            map_src: (0, 0),
            stats: VideoStats::default(),
        }
    }

    /// Open `spec`. **Never blocks, and never takes a run down with it**: a
    /// destination that cannot be written to right now yields an armed-but-
    /// waiting emitter or a disabled one plus an explanation, never a wait.
    ///
    /// Two destinations get a *specific* answer rather than a bare OS error,
    /// because both are things a user will reasonably try:
    ///
    /// * a `/dev/video*` node — writing raw bytes at a V4L2 device does not
    ///   work until its format has been negotiated; the supported route is
    ///   `ffmpeg -f v4l2` (see [`v4l2`]).
    /// * a **FIFO** — `open` for writing blocks until a reader appears, which
    ///   is why this one is opened `O_NONBLOCK` and left in
    ///   [`StreamState::Waiting`] until a consumer turns up. That used to be a
    ///   hang at startup with no UI and no way out but `kill`.
    // The V4L2 branch below is `#[cfg(all(target_os = "linux", feature =
    // "libc"))]`, so on a build without it (the embeddable core, and every
    // non-Linux target) `spec` is never rewritten and the two `if`s have
    // nothing between them. Both lints are therefore about code that is not
    // compiled rather than about this code.
    #[allow(unused_mut, clippy::collapsible_if)]
    pub fn open(spec: VideoSpec) -> (VideoOut, Option<String>) {
        let mut spec = spec;
        let mut note = None;
        if let Destination::Path(p) = spec.dest.clone() {
            if v4l2::is_video_device(&p) {
                if !v4l2::NATIVE_OUTPUT {
                    return (
                        VideoOut::disabled(),
                        Some(v4l2::direct_write_refusal(&p, &spec)),
                    );
                }
                #[cfg(all(target_os = "linux", feature = "libc"))]
                {
                    // **Detection over configuration.** A camera has exactly one
                    // useful shape, and the user should not have to know it:
                    // anything but YUYV is not what the device negotiates, and
                    // 160x144 is legal V4L2 that consumers reject. So both are
                    // set here rather than left as a default that cannot work —
                    // which is precisely the failure mode that fed a 160x144
                    // rgba stream to a bridge expecting 640x480 yuyv.
                    // Both of the notes below matter and neither may swallow
                    // the other: "your format was overridden" and "the device
                    // is not here yet" are different things to fix.
                    let mut notes: Vec<String> = Vec::new();
                    // Only worth saying when the user asked for something a
                    // camera cannot be: RGBA is merely the module default, and
                    // complaining about a value nobody typed is how a useful
                    // warning becomes noise people stop reading.
                    if !matches!(spec.format, PixelFormat::Yuyv422 | PixelFormat::Rgba) {
                        notes.push(format!(
                            "video: {} is a camera, so the stream is YUYV — \
                             `video_format = {}` does not apply to a V4L2 device",
                            p.display(),
                            spec.format.ffmpeg_name(),
                        ));
                    }
                    // Whatever was asked for, a V4L2 node gets YUYV — that is
                    // what the driver negotiates and the only thing the
                    // converter emits for it.
                    spec.format = PixelFormat::Yuyv422;
                    if spec.size.is_none() {
                        spec.size = Some(v4l2::DEFAULT_SIZE);
                    }
                    let mut d = v4l2::sink::V4l2Sink::new(p.clone());
                    d.attach(
                        spec.size.unwrap_or(v4l2::DEFAULT_SIZE),
                        Instant::now(),
                        RETRY_EVERY,
                    );
                    if let Some(n) = d.format() {
                        spec.size = Some((n.width, n.height));
                        // Checked against what the *driver* answered, not what
                        // was asked for: v4l2loopback can be created with fixed
                        // caps, so the geometry that reaches the consumer is
                        // not always the one in the config file.
                        notes.extend(v4l2::aspect_warning(n.width, n.height));
                    } else {
                        notes.extend(
                            spec.size.and_then(|(w, h)| v4l2::aspect_warning(w, h)),
                        );
                        // Not usable *yet*. The emulator still starts, and the
                        // camera is picked up if it appears — but if the module
                        // is simply not installed, that is the one remaining
                        // setup step and it has to be actionable.
                        let why = d.last_error.clone().unwrap_or_else(|| {
                            format!("{} is not available", p.display())
                        });
                        notes.push(if !d.last_error_transient {
                            // Cannot ever succeed as configured (a real webcam).
                            // The message already names the alternatives; do not
                            // also promise a retry that can never work.
                            format!("video: {why}")
                        } else if !v4l2::module_loaded() {
                            format!("video: {why}\n{}", v4l2::install_hint_for_missing())
                        } else {
                            // Naming the device the user already asked for is
                            // not help; list the alternatives only.
                            let others: Vec<_> = v4l2::loopback_devices()
                                .into_iter()
                                .filter(|(n, _)| Path::new(n) != p)
                                .collect();
                            let known = if others.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    " Cameras on this machine: {}",
                                    others
                                        .iter()
                                        .map(|(n, l)| format!("{n} (\"{l}\")"))
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                )
                            };
                            format!(
                                "video: {why} — retrying while the stream is on.{known}"
                            )
                        });
                    }
                    let mut v = VideoOut::disabled();
                    if d.live() {
                        v.stats.attaches = 1;
                        v.out = spec.size;
                    }
                    v.spec = spec;
                    v.sink = Some(Sink::V4l2(d));
                    let note = (!notes.is_empty()).then(|| notes.join("\n"));
                    return (v, note);
                }
            }
        }
        let sink = match &spec.dest {
            Destination::Stdout => {
                pipe::ignore_sigpipe();
                Ok(Sink::Stdout(std::io::stdout()))
            }
            Destination::Path(p) if is_fifo(p) => {
                if !pipe::SUPPORTED {
                    Err(format!(
                        "video: {} is a FIFO, and this build cannot open one without \
                         blocking until a consumer attaches (no `libc` feature). Stream \
                         to a file instead, or build with the default features.",
                        p.display()
                    ))
                } else {
                    pipe::ignore_sigpipe();
                    let mut f = Fifo::new(p.clone());
                    f.reconnect(Instant::now());
                    if !f.live() {
                        note = Some(format!(
                            "video: armed on {} — waiting for a consumer, streaming \
                             starts the moment one attaches:\n  {}",
                            p.display(),
                            spec.ffmpeg_hint(None),
                        ));
                    }
                    Ok(Sink::Fifo(f))
                }
            }
            Destination::Path(p) => std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(p)
                .map(Sink::File)
                .map_err(|e| format!("video: cannot open {}: {e}", p.display())),
        };
        match sink {
            Ok(s) => {
                let mut v = VideoOut::disabled();
                if let Sink::Fifo(f) = &s {
                    if f.live() {
                        v.stats.attaches = 1;
                    }
                }
                v.spec = spec;
                v.sink = Some(s);
                v.write_descriptor();
                (v, note)
            }
            Err(e) => (VideoOut::disabled(), Some(e)),
        }
    }

    /// An emitter that keeps every byte in memory (tests).
    pub fn in_memory(spec: VideoSpec) -> VideoOut {
        let mut v = VideoOut::disabled();
        v.spec = spec;
        v.sink = Some(Sink::Memory(Vec::new()));
        v
    }

    /// `true` when the stream is **armed** — configured, opened and being fed.
    /// Callers check this before doing any work that is not free, including
    /// reading the clock.
    ///
    /// Armed is not the same as reaching a consumer: a FIFO with nothing on the
    /// far end is armed and [`StreamState::Waiting`], because it must keep
    /// being offered frames to notice a consumer arriving. Ask [`VideoOut::state`]
    /// for the thing a user cares about.
    pub fn is_enabled(&self) -> bool {
        self.sink.is_some() && self.streaming
    }

    /// Off, waiting for a consumer, or live. The answer a status row shows.
    pub fn state(&self) -> StreamState {
        if !self.streaming {
            return StreamState::Off;
        }
        match &self.sink {
            None => StreamState::Off,
            Some(Sink::Fifo(f)) if !f.live() => StreamState::Waiting,
            #[cfg(all(target_os = "linux", feature = "libc"))]
            Some(Sink::V4l2(d)) if !d.live() => StreamState::Waiting,
            Some(_) => StreamState::Live,
        }
    }

    /// Stop sending the **game** without giving up the destination — the
    /// camera keeps existing, and keeps being fed the idle card.
    ///
    /// **The device is a persistent output; streaming the game into it is a
    /// mode.** Those are different things, and collapsing them is what made `v`
    /// destructive: stopping the stream killed the camera, so a viewer in the
    /// middle of a call lost the device and had to re-pick it.
    ///
    /// It also could not be undone. `v4l2loopback` is built with
    /// `exclusive_caps=1` on every distribution that ships it, and that flag
    /// means the node advertises `V4L2_CAP_VIDEO_OUTPUT` only while it has no
    /// consumer: the instant the producer lets go of a device somebody is
    /// watching, `VIDIOC_QUERYCAP` on a new fd answers CAPTURE and `VIDIOC_S_FMT`
    /// with `V4L2_BUF_TYPE_VIDEO_OUTPUT` answers `EINVAL`, for as long as the
    /// viewer stays open. Measured here: on/off/on worked once and then the
    /// camera was gone. **Never releasing it dissolves that problem rather than
    /// working around it** — there is nothing left to re-acquire.
    ///
    /// Off still means the game is not going out: no picture, the status row
    /// says so, and telemetry counts no streamed frames. What continues is the
    /// device and a card at [`IDLE_PERIOD`], because a consumer that stops
    /// receiving frames entirely is a consumer that eventually drops the device
    /// — which would be the same failure by a slower route.
    pub fn go_idle(&mut self) {
        self.streaming = false;
        self.card_staged = false;
        self.idle_next = None;
    }

    /// Send the game again, down a destination this stream never let go of.
    pub fn resume(&mut self) {
        self.streaming = true;
        self.card_staged = false;
        self.idle_next = None;
        // The wall clock ran on while the game feed was off. Restarting the
        // pacer here rather than letting `pump` discover a deadline minutes in
        // the past is the difference between resuming and reporting a resync
        // (and a burst of duplicates) on every press of `v`.
        self.next = None;
    }

    /// Is the destination being held open and fed the idle card?
    pub fn is_idle(&self) -> bool {
        self.sink.is_some() && !self.streaming
    }

    /// Must this destination be held open across a stop rather than closed?
    ///
    /// **Yes for anything live, no for a recording** — and that is exactly the
    /// distinction [`Intent`] already draws. A file has to be closed to be a
    /// finished recording at all; a camera, a FIFO or a pipe has something on
    /// the far end that must not be disturbed, and for the FIFO that "something"
    /// is very often `tools/virtual-camera.sh`, whose `ffmpeg` owns the camera —
    /// so an EOF there kills the device just as surely, one process further out.
    pub fn must_stay_open(&self) -> bool {
        self.sink.is_some() && self.intent() == Intent::Stream
    }

    /// Recording a file, or feeding a live consumer. See the module docs.
    pub fn intent(&self) -> Intent {
        match &self.sink {
            Some(Sink::Fifo(_)) | Some(Sink::Stdout(_)) => Intent::Stream,
            #[cfg(all(target_os = "linux", feature = "libc"))]
            Some(Sink::V4l2(_)) => Intent::Stream,
            _ => self.spec.dest.intent(),
        }
    }

    /// Is this stream a camera the operating system is publishing?
    ///
    /// Worth its own question because it changes what "live" *means*: a camera
    /// is being fed whether or not anyone has opened it, so there is no third
    /// party to wait for once the device is negotiated.
    pub fn is_camera(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "libc"))]
        {
            matches!(self.sink, Some(Sink::V4l2(_)))
        }
        #[cfg(not(all(target_os = "linux", feature = "libc")))]
        false
    }

    /// Why a camera is not live yet, if it is not.
    pub fn device_error(&self) -> Option<String> {
        #[cfg(all(target_os = "linux", feature = "libc"))]
        if let Some(Sink::V4l2(d)) = &self.sink {
            return d.last_error.clone();
        }
        None
    }

    /// Where the frames are going, for a status row or a toast.
    pub fn destination(&self) -> String {
        self.spec.dest.label()
    }

    pub fn spec(&self) -> &VideoSpec {
        &self.spec
    }

    pub fn stats(&self) -> VideoStats {
        self.stats
    }

    /// Bytes captured by an in-memory sink.
    pub fn captured(&self) -> &[u8] {
        match &self.sink {
            Some(Sink::Memory(v)) => v,
            _ => &[],
        }
    }

    /// The output geometry, once the first frame has fixed it.
    pub fn geometry(&self) -> Option<(u32, u32)> {
        self.out
    }

    /// Offer an emulated frame. `rgba` is `w * h * 4` bytes.
    pub fn frame(&mut self, rgba: &[u8], w: u32, h: u32, now: Instant) {
        if !self.is_enabled() {
            return;
        }
        self.stage(rgba, w, h);
        self.pump(now, true);
    }

    /// No frame was produced this tick (paused, or under a modal overlay).
    /// Keeps a `Realtime` stream at its contracted rate by repeating the last
    /// picture — a consumer that stops receiving bytes stalls, and a recording
    /// made across a pause would otherwise be shorter than the pause.
    pub fn idle(&mut self, now: Instant) {
        if self.sink.is_none() {
            return;
        }
        if !self.streaming {
            self.tick_idle_card(now);
            return;
        }
        // A FIFO may still owe a consumer the rest of a frame. Push it here
        // whatever the pacing mode: `Pace::Emulated` writes only on a fresh
        // frame, so without this a consumer that resumed after a stall would
        // sit on a partial frame until the emulator produced another one.
        if let Some(Sink::Fifo(p)) = self.sink.as_mut() {
            if p.owes_tail() {
                match p.service() {
                    Push::Stalled => self.stats.stalled += 1,
                    Push::Disconnected => self.stats.detaches += 1,
                    _ => {}
                }
            }
        }
        if self.staged.is_empty() {
            return;
        }
        self.pump(now, false);
    }

    /// Keep an idling destination alive with the card, at [`IDLE_PERIOD`].
    ///
    /// Drawn at native resolution into **the rectangle the game occupies**, not
    /// edge to edge. A full-bleed card and a letterboxed game are two different
    /// pictures at two different sizes, and switching between them made the
    /// feed jump — the frame appeared to grow and shrink every time `v` was
    /// pressed. Same box, same bars, so only the content changes.
    ///
    /// Still drawn at the *fitted* size rather than 160×144 magnified, so the
    /// text is crisp at whatever the device negotiated — this is the picture
    /// strangers see mid-stream. Converted once and then re-written unchanged,
    /// so a tick costs one `write(2)`.
    fn tick_idle_card(&mut self, now: Instant) {
        let due = *self.idle_next.get_or_insert(now);
        if now < due {
            return;
        }
        self.idle_next = Some(now + IDLE_PERIOD);
        let (w, h) = self.card_size();
        // Re-stage on a size change as well as on a switch from the game: the
        // driver can adjust the format on a re-attach while the stream is
        // idling, and a card left at the old size would be letterboxed inside
        // the new one — bars inside bars.
        if !self.card_staged || self.card_dims != (w, h) {
            if self.card_dims != (w, h) || self.card.is_empty() {
                self.card.clear();
                self.card.resize(w as usize * h as usize * 4, 0);
                card::draw_idle(&mut self.card, w, h);
                self.card_dims = (w, h);
            }
            self.staged.clear();
            self.staged.extend_from_slice(&self.card);
            self.staged_dims = (w, h);
            self.wire_valid = false;
            self.card_staged = true;
        }
        self.emit(now);
    }

    /// The rectangle the idle card is drawn at: exactly the one the game lands
    /// in, so the two are interchangeable inside the frame.
    ///
    /// `game_fit` is the last game frame's fitted rectangle when there has been
    /// one; before that, the size a 160×144 frame would fit at. Both go through
    /// [`fit_size`], so the card can never be the thing that crops.
    fn card_size(&self) -> (u32, u32) {
        let Some((ow, oh)) = self.out.or(self.spec.size) else {
            return (SRC_W, SRC_H);
        };
        let (sw, sh) = self.game_fit.unwrap_or((SRC_W, SRC_H));
        let (tw, th) = fit_size(sw, sh, ow, oh);
        if tw == 0 || th == 0 {
            (SRC_W, SRC_H)
        } else {
            (tw, th)
        }
    }

    fn stage(&mut self, rgba: &[u8], w: u32, h: u32) {
        // A game frame after an idle card (or vice versa) is a different
        // picture at a possibly different size; the cached wire bytes and the
        // scaling tables both have to go.
        self.card_staged = false;
        if self.staged.len() != rgba.len() {
            self.staged.resize(rgba.len(), 0);
        }
        self.staged.copy_from_slice(rgba);
        self.staged_dims = (w, h);
        // Remembered so the idle card lands in the same rectangle. It is the
        // *source* size, not the fitted one: the output geometry can still
        // change under it (the driver adjusts the format on a re-attach), and
        // `card_size` re-fits from this every time.
        self.game_fit = Some((w, h));
        self.wire_valid = false;
    }

    fn pump(&mut self, now: Instant, fresh: bool) {
        match self.spec.pace {
            Pace::Emulated => {
                if fresh {
                    self.emit(now);
                }
            }
            Pace::Realtime => {
                let due = *self.next.get_or_insert(now);
                if now + SLACK < due {
                    // Ahead of wall clock: this frame is superseded by the next
                    // one. That is fast-forward behaving as documented.
                    if fresh {
                        self.stats.dropped += 1;
                    }
                    return;
                }
                let mut deadline = due;
                let mut n = 0;
                while now + SLACK >= deadline && n < MAX_BURST {
                    deadline += FRAME_PERIOD;
                    n += 1;
                }
                if now + SLACK >= deadline {
                    // Hopelessly behind (suspend, a long stall): resync instead
                    // of dumping a burst of duplicates.
                    self.stats.resyncs += 1;
                    deadline = now + FRAME_PERIOD;
                }
                self.next = Some(deadline);
                for i in 0..n {
                    if i > 0 || !fresh {
                        self.stats.duplicated += 1;
                    }
                    self.emit(now);
                }
            }
        }
    }

    /// Convert the staged frame (once) and write it.
    ///
    /// The one hard rule: this returns in bounded time no matter what the
    /// consumer is doing. It is called from inside the emulated frame, so a
    /// blocking write here *is* frame time — the same mistake the audio player
    /// made (see `docs/audio.md` §4c).
    fn emit(&mut self, now: Instant) {
        let t = Instant::now();
        // Waiting for a consumer: try to attach, but do not pay for a pixel
        // conversion nobody is going to read. This is the common case for a
        // stream armed from `config.toml` before the bridge is started.
        if let Some(Sink::Fifo(p)) = self.sink.as_mut() {
            if !p.live() {
                p.reconnect(now);
                if !p.live() {
                    self.stats.unconsumed += 1;
                    self.stats.last_ms = t.elapsed().as_secs_f32() * 1000.0;
                    return;
                }
                self.stats.attaches += 1;
            }
        }
        // The camera is the same shape of problem — the device may be missing
        // (v4l2loopback not loaded yet) or busy (another producer holds it),
        // and both must be a state rather than a failure. The difference is
        // that the *driver* decides the geometry, so it is adopted here rather
        // than fixed by the first frame.
        #[cfg(all(target_os = "linux", feature = "libc"))]
        {
            let want = self.spec.size.unwrap_or(v4l2::DEFAULT_SIZE);
            let mut adopt = None;
            if let Some(Sink::V4l2(d)) = self.sink.as_mut() {
                let was = d.live();
                if !was {
                    d.attach(want, now, RETRY_EVERY);
                    if !d.live() {
                        self.stats.unconsumed += 1;
                        self.stats.last_ms = t.elapsed().as_secs_f32() * 1000.0;
                        return;
                    }
                    self.stats.attaches += 1;
                }
                adopt = d.format().map(|n| (n.width, n.height));
            }
            if let Some(geo) = adopt {
                if self.out != Some(geo) {
                    // The driver adjusted the format (or this is the first
                    // attach). What it said is what the frames must be — asking
                    // for 640x480 and writing 640x480 when it answered
                    // something else is how a camera shows torn diagonal mush.
                    self.out = Some(geo);
                    self.spec.size = Some(geo);
                    self.xmap.clear();
                    self.ymap.clear();
                    self.wire_valid = false;
                }
            }
        }
        if !self.wire_valid {
            let unknown_geometry = self.out.is_none();
            self.convert();
            self.wire_valid = true;
            // The first frame fixes the geometry for the life of the stream, so
            // that is the moment the descriptor becomes exact. One extra write.
            if unknown_geometry {
                self.write_descriptor();
            }
        }
        let mut wrote = true;
        let ok = match self.sink.as_mut() {
            Some(Sink::Stdout(o)) => {
                let mut lock = o.lock();
                lock.write_all(&self.wire)
                    .and_then(|_| lock.flush())
                    .is_ok()
            }
            Some(Sink::File(f)) => f.write_all(&self.wire).is_ok(),
            Some(Sink::Fifo(p)) => {
                match p.push(&self.wire, now) {
                    Push::Wrote => {}
                    Push::NoConsumer => {
                        self.stats.unconsumed += 1;
                        wrote = false;
                    }
                    Push::Stalled => {
                        self.stats.stalled += 1;
                        wrote = false;
                    }
                    Push::Disconnected => {
                        self.stats.detaches += 1;
                        wrote = false;
                    }
                }
                // A FIFO is never switched off by any of these: every one of
                // them is recoverable, and the next consumer must be picked up.
                true
            }
            #[cfg(all(target_os = "linux", feature = "libc"))]
            Some(Sink::V4l2(d)) => {
                match d.put(&self.wire) {
                    v4l2::sink::Put::Wrote => {}
                    // The driver's buffers are full: whatever is watching the
                    // camera is not draining them. Drop the frame.
                    v4l2::sink::Put::Busy => {
                        self.stats.stalled += 1;
                        wrote = false;
                    }
                    v4l2::sink::Put::Lost => {
                        self.stats.detaches += 1;
                        wrote = false;
                    }
                    v4l2::sink::Put::NoDevice => {
                        self.stats.unconsumed += 1;
                        wrote = false;
                    }
                    // Our frame does not match what the driver negotiated. Not
                    // a device condition — a bug on our side, so it is counted
                    // as a write error rather than swallowed as "slow".
                    v4l2::sink::Put::WrongSize => {
                        self.stats.write_errors += 1;
                        wrote = false;
                    }
                }
                // Never switched off: a device that goes away can come back,
                // exactly like a FIFO's consumer.
                true
            }
            Some(Sink::Memory(v)) => {
                v.extend_from_slice(&self.wire);
                true
            }
            None => true,
        };
        if !ok {
            // A closed pipe (the consumer quit) must not take the game down and
            // must not cost a syscall per frame for the rest of the run.
            self.stats.write_errors += 1;
            self.sink = None;
        } else if wrote {
            // Idle-card frames are counted apart from the game's. `frames` is
            // the number telemetry's `video.frames ≈ elapsed × 59.7275`
            // invariant is written against, and a card at 2 Hz would quietly
            // make that assertion meaningless.
            if self.card_staged {
                self.stats.idle_frames += 1;
            } else {
                self.stats.frames += 1;
            }
            self.stats.bytes += self.wire.len() as u64;
        }
        self.stats.last_ms = t.elapsed().as_secs_f32() * 1000.0;
    }

    fn convert(&mut self) {
        let (sw, sh) = self.staged_dims;
        let (ow, oh) = *self.out.get_or_insert_with(|| {
            let (w, h) = self.spec.size.unwrap_or((sw, sh));
            // The geometry is announced to the consumer once; an odd width
            // would desynchronise every 4:2:2 row.
            if self.spec.format.needs_even_width() && w % 2 == 1 {
                (w + 1, h)
            } else {
                (w, h)
            }
        });

        let src: &[u8] = if (sw, sh) == (ow, oh) {
            &self.staged
        } else {
            // Keyed on the **source** size as well as the output: the idle card
            // and a `display`-source game frame are different sizes going to the
            // same output, and reusing one's tables for the other tears the
            // picture.
            if self.map_src != (sw, sh)
                || self.xmap.len() != ow as usize
                || self.ymap.len() != oh as usize
            {
                let (xm, ym) = fit_maps(sw, sh, ow, oh);
                self.xmap = xm;
                self.ymap = ym;
                self.map_src = (sw, sh);
            }
            letterbox(
                &self.staged,
                sw,
                &mut self.fitted,
                &mut self.row,
                ow,
                oh,
                &self.xmap,
                &self.ymap,
            );
            &self.fitted
        };

        match self.spec.format {
            PixelFormat::Rgba => {
                if self.wire.len() != src.len() {
                    self.wire.resize(src.len(), 0);
                }
                self.wire.copy_from_slice(src);
            }
            PixelFormat::Rgb24 => rgba_to_rgb24(src, &mut self.wire),
            PixelFormat::Yuyv422 => rgba_to_yuyv422(src, ow, oh, &mut self.wire),
        }
    }

    /// Publish the sidecar descriptor beside a FIFO destination, so the
    /// consumer is configured from the stream rather than from a second copy of
    /// the same numbers. See [`descriptor`].
    ///
    /// Best-effort by design: an unwritable directory costs the convenience,
    /// never the stream. Nothing is written beside a *recording* — a file
    /// already carries its geometry and format in its name
    /// ([`recording_name`]).
    fn write_descriptor(&self) {
        let Some(Sink::Fifo(f)) = self.sink.as_ref() else {
            return;
        };
        let _ =
            std::fs::write(descriptor_path(&f.path), descriptor(&self.spec, self.out));
    }

    /// The exact command that consumes this stream. Printed at startup so the
    /// answer to "how do I read this?" ships with the stream itself.
    pub fn ffmpeg_hint(&self) -> String {
        self.spec.ffmpeg_hint(self.out)
    }

    /// One line describing the live stream, for the frontend to show.
    ///
    /// A camera that has not attached must not be described as pickable — the
    /// whole failure this work exists to remove is a banner that reads like
    /// everything is fine while nothing is being delivered.
    pub fn describe(&self) -> String {
        // A *parked* camera is not an unavailable one — it is one nobody has
        // asked for frames yet. Only complain about a camera we are trying to
        // feed and cannot.
        if self.is_camera() && self.streaming && self.state() != StreamState::Live {
            return match self.device_error() {
                Some(why) => format!("video: camera not available — {why}"),
                None => {
                    format!("video: camera {} not available yet", self.spec.dest.label())
                }
            };
        }
        self.spec.describe(self.out)
    }

    pub fn flush(&mut self) {
        match self.sink.as_mut() {
            Some(Sink::Stdout(o)) => {
                let _ = o.lock().flush();
            }
            Some(Sink::File(f)) => {
                let _ = f.flush();
            }
            // A FIFO is written unbuffered and non-blocking; there is nothing
            // held back here, and blocking to push a tail out would reintroduce
            // exactly the stall this sink exists to avoid.
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing into a frame: a bitmap font, and the idle card
// ---------------------------------------------------------------------------

/// Everything needed to put legible text into an RGBA frame, with no
/// dependency and no vendored asset.
///
/// **Licence, deliberately:** the font below is a 5×7 cell font written by hand
/// for this repository, so it is the repository's own MPL-2.0 code. That is not
/// laziness about finding a good font — it is the only option with *no* licence
/// question at all, and 5×7 uppercase is a solved shape with nothing to
/// improve on. Vendoring a public-domain or OFL bitmap font would have been
/// acceptable (see `CONTRIBUTING.md`), and pulling a shaping library for ASCII
/// glyph blitting would not.
///
/// Written to be shared: the idle card uses it now, and compositing the status
/// panel into the feed needs exactly this and a layout pass on top.
pub mod card {
    /// Glyph cell, in pixels, before scaling. One byte per row, bit 4 is the
    /// leftmost column.
    pub const GLYPH_W: u32 = 5;
    pub const GLYPH_H: u32 = 7;
    /// Cell advance including the one-pixel gap between characters.
    pub const ADVANCE: u32 = GLYPH_W + 1;

    /// The classic DMG screen palette, darkest first. The card is unmistakably
    /// a Game Boy without needing a picture of one.
    pub const DMG_DARKEST: [u8; 4] = [0x0F, 0x38, 0x0F, 0xFF];
    pub const DMG_DARK: [u8; 4] = [0x30, 0x62, 0x30, 0xFF];
    pub const DMG_LIGHT: [u8; 4] = [0x8B, 0xAC, 0x0F, 0xFF];
    pub const DMG_LIGHTEST: [u8; 4] = [0x9B, 0xBC, 0x0F, 0xFF];

    /// `A`-`Z`, `0`-`9` and the punctuation the cards need. Anything else is
    /// drawn as a blank cell rather than a tofu box — a missing glyph must not
    /// be the most eye-catching thing on a public stream.
    fn glyph(c: char) -> [u8; GLYPH_H as usize] {
        match c.to_ascii_uppercase() {
            'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
            'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
            'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
            'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
            'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
            'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
            'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],
            'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
            'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
            'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
            'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
            'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
            'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
            'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
            'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
            'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
            'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
            'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
            'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
            'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
            'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
            'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
            'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
            'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
            'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
            'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
            '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
            '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
            '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
            '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
            '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
            '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
            '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
            '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
            '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
            '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
            '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
            '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
            ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
            '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
            _ => [0; GLYPH_H as usize],
        }
    }

    /// Width in pixels of `text` at `scale`.
    pub fn text_width(text: &str, scale: u32) -> u32 {
        match text.chars().count() as u32 {
            0 => 0,
            n => (n * ADVANCE - 1) * scale,
        }
    }

    /// The largest integer scale at which `text` still fits `target` pixels.
    /// Legibility survives re-encoding by being *big*, so text is sized to the
    /// space it has rather than to a constant nobody re-checks.
    pub fn fit_scale(text: &str, target: u32) -> u32 {
        let unit = text_width(text, 1).max(1);
        (target / unit).max(1)
    }

    /// An RGBA frame being drawn into.
    ///
    /// Carrying the target as one value rather than three loose arguments is
    /// what keeps the drawing calls readable — and it is the shape the panel
    /// compositor wants too, where a sub-rectangle of the frame is handed to a
    /// layout pass.
    pub struct Canvas<'a> {
        pub buf: &'a mut [u8],
        pub w: u32,
        pub h: u32,
    }

    impl<'a> Canvas<'a> {
        pub fn new(buf: &'a mut [u8], w: u32, h: u32) -> Canvas<'a> {
            Canvas { buf, w, h }
        }

        /// One pixel, if it is inside the frame.
        #[inline]
        fn put(&mut self, x: i64, y: i64, c: [u8; 4]) {
            if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
                return;
            }
            let i = (y as usize * self.w as usize + x as usize) * 4;
            if i + 4 <= self.buf.len() {
                self.buf[i..i + 4].copy_from_slice(&c);
            }
        }

        /// A filled rectangle, clipped to the frame.
        pub fn fill(&mut self, x: i64, y: i64, rw: u32, rh: u32, c: [u8; 4]) {
            for dy in 0..rh as i64 {
                for dx in 0..rw as i64 {
                    self.put(x + dx, y + dy, c);
                }
            }
        }

        /// `text` with its top-left corner at `(x, y)`, magnified by `scale`.
        pub fn text(&mut self, x: i64, y: i64, text: &str, scale: u32, c: [u8; 4]) {
            let s = scale.max(1);
            for (i, ch) in text.chars().enumerate() {
                let g = glyph(ch);
                let gx = x + (i as u32 * ADVANCE * s) as i64;
                for (row, bits) in g.iter().enumerate() {
                    for col in 0..GLYPH_W {
                        if bits & (1 << (GLYPH_W - 1 - col)) == 0 {
                            continue;
                        }
                        self.fill(
                            gx + (col * s) as i64,
                            y + (row as u32 * s) as i64,
                            s,
                            s,
                            c,
                        );
                    }
                }
            }
        }

        /// `text` centred horizontally, at `y`.
        pub fn text_centred(&mut self, y: i64, text: &str, scale: u32, c: [u8; 4]) {
            let x = (self.w as i64 - text_width(text, scale) as i64) / 2;
            self.text(x, y, text, scale, c);
        }
    }

    /// The picture the camera shows when the game feed is switched off.
    ///
    /// Drawn at the frame's **own** size rather than at 160×144 and magnified,
    /// so the text is crisp at whatever the device negotiated instead of being
    /// nearest-neighbour blocks — this is what strangers see mid-stream, and it
    /// has to look deliberate rather than broken.
    ///
    /// Deliberately says nothing about the machine it is running on: no game
    /// title, no path, no host. Switching the feed off is very often *exactly*
    /// the moment someone does not want what they are doing on screen.
    pub fn draw_idle(buf: &mut [u8], w: u32, h: u32) {
        let mut c = Canvas::new(buf, w, h);
        let unit = (w / 160).min(h / 144).max(1);
        c.fill(0, 0, w, h, DMG_DARKEST);
        // A frame, inset, so the card reads as a designed screen and not as a
        // failure to paint the background.
        let b = unit * 2;
        let m = unit * 6;
        c.fill(m as i64, m as i64, w - 2 * m, b, DMG_DARK);
        c.fill(m as i64, (h - m - b) as i64, w - 2 * m, b, DMG_DARK);
        c.fill(m as i64, m as i64, b, h - 2 * m, DMG_DARK);
        c.fill((w - m - b) as i64, m as i64, b, h - 2 * m, DMG_DARK);

        const NAME: &str = "TERMINALGB";
        const STATE: &str = "STREAM PAUSED";
        const HINT: &str = "PRESS V TO RESUME";
        let ns = fit_scale(NAME, w * 7 / 10);
        let ss = fit_scale(STATE, w * 11 / 20);
        let hs = fit_scale(HINT, w / 2);

        // Vertical rhythm from the block heights themselves, so the card is
        // balanced at 160x144 and at 1280x720 without a table of magic numbers.
        let nh = GLYPH_H * ns;
        let sh = GLYPH_H * ss;
        let hh = GLYPH_H * hs;
        let gap = (GLYPH_H * ss) as i64;
        let total = nh as i64 + gap + sh as i64 + gap / 2 + hh as i64;
        let mut y = (h as i64 - total) / 2;

        c.text_centred(y, NAME, ns, DMG_LIGHTEST);
        y += nh as i64 + gap / 3;
        // A rule under the name, the width of the name.
        let rule = text_width(NAME, ns);
        c.fill((w as i64 - rule as i64) / 2, y, rule, unit.max(1), DMG_DARK);
        y += gap * 2 / 3;
        c.text_centred(y, STATE, ss, DMG_LIGHT);
        y += sh as i64 + gap / 2;
        // Not `DMG_DARK`: a low-contrast line is the first thing a stream
        // encoder throws away, and this one is the instruction. Hierarchy comes
        // from size here, not from fading text into the background.
        c.text_centred(y, HINT, hs, DMG_LIGHT);

        // Nothing else. A row of palette swatches was tried here and cut: on a
        // dark card the darkest shade is invisible and on a light backing the
        // lightest one is, so it read as three squares and a smudge — decoration
        // that has to be explained is worse than none.
    }
}

// ---------------------------------------------------------------------------
// Geometry and pixel conversion (pure, so they are directly testable)
// ---------------------------------------------------------------------------

/// Nearest-neighbour source-index tables for fitting `sw×sh` inside `dw×dh`.
///
/// Integer magnification is preferred whenever it fits: pixel art scaled by a
/// non-integer factor gets uneven pixel widths, which shimmers. The remainder
/// becomes black bars (letterbox/pillarbox) — the aspect ratio is never
/// stretched and the picture is never cropped, because 160×144 is 10:9 and
/// every conventional camera size is 4:3 or 16:9.
pub fn fit_maps(sw: u32, sh: u32, dw: u32, dh: u32) -> (Vec<u32>, Vec<u32>) {
    let (tw, th) = fit_size(sw, sh, dw, dh);
    let (ox, oy) = fit_origin(tw, th, dw, dh);
    // `u32::MAX` marks a bar pixel (no source).
    let mut xmap = vec![u32::MAX; dw as usize];
    let mut ymap = vec![u32::MAX; dh as usize];
    // Sampled at the CENTRE of each output pixel, not its left edge:
    // `(2x+1)*s / 2t` rather than `x*s / t`. For an integer magnification the
    // two are identical, so the crisp x3/x5 path is untouched — but when the
    // frame is smaller than the source, edge sampling never reaches the last
    // source column or row (160 columns into 100 stops at 158), which quietly
    // shaves the outermost pixels off a picture that is supposed to be whole.
    let sample = |i: u32, s: u32, t: u32| -> u32 {
        (((2 * i as u64 + 1) * s as u64) / (2 * t as u64)).min(s as u64 - 1) as u32
    };
    for x in 0..tw {
        xmap[(ox + x) as usize] = sample(x, sw, tw);
    }
    for y in 0..th {
        ymap[(oy + y) as usize] = sample(y, sh, th);
    }
    (xmap, ymap)
}

/// Where the fitted picture's top-left corner sits inside `dw×dh`.
///
/// Pixels are whole, so an odd leftover cannot be split evenly: the extra
/// column/row goes to the right and bottom bar. That is a half-pixel bias, the
/// smallest possible, and it is *fixed* — the alternative, rounding up, moves
/// the picture by a pixel whenever the geometry changes parity, which reads as
/// a jitter on a live feed.
fn fit_origin(tw: u32, th: u32, dw: u32, dh: u32) -> (u32, u32) {
    (dw.saturating_sub(tw) / 2, dh.saturating_sub(th) / 2)
}

/// The size the picture is drawn at inside `dw×dh`.
///
/// Never larger than `dw×dh` in either axis — this function is the whole of the
/// "fit, never crop" promise, so every branch is bounded by the destination.
pub fn fit_size(sw: u32, sh: u32, dw: u32, dh: u32) -> (u32, u32) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return (0, 0);
    }
    let k = (dw / sw).min(dh / sh);
    if k >= 1 {
        return (sw * k, sh * k);
    }
    // Smaller than the source: fall back to an aspect-preserving fractional
    // fit (uneven pixels, but a legible picture beats a cropped one). 64-bit
    // arithmetic because `dw * sh` overflows a `u32` past about 65k×65k, and a
    // wrapped comparison here would pick the wrong axis and crop.
    let (sw64, sh64) = (sw as u64, sh as u64);
    let (dw64, dh64) = (dw as u64, dh as u64);
    if dw64 * sh64 <= dh64 * sw64 {
        (dw, ((dw64 * sh64 / sw64) as u32).clamp(1, dh))
    } else {
        (((dh64 * sw64 / sh64) as u32).clamp(1, dw), dh)
    }
}

/// Scale + centre `src` into `dst` using precomputed index maps. Bars are
/// opaque black. Each distinct source row is built once and memcpy'd to every
/// output row that repeats it.
#[allow(clippy::too_many_arguments)]
pub fn letterbox(
    src: &[u8],
    sw: u32,
    dst: &mut Vec<u8>,
    row: &mut Vec<u8>,
    dw: u32,
    dh: u32,
    xmap: &[u32],
    ymap: &[u32],
) {
    let stride = dw as usize * 4;
    if dst.len() != stride * dh as usize {
        dst.resize(stride * dh as usize, 0);
    }
    if row.len() != stride {
        row.resize(stride, 0);
    }
    // Bars: opaque black everywhere, overwritten where the picture lands.
    for px in dst.chunks_exact_mut(4) {
        px.copy_from_slice(&[0, 0, 0, 255]);
    }
    let mut built: Option<u32> = None;
    for (y, &sy) in ymap.iter().enumerate() {
        if sy == u32::MAX {
            continue;
        }
        if built != Some(sy) {
            let base = sy as usize * sw as usize * 4;
            for (x, &sx) in xmap.iter().enumerate() {
                let d = x * 4;
                if sx == u32::MAX {
                    row[d..d + 4].copy_from_slice(&[0, 0, 0, 255]);
                } else {
                    let s = base + sx as usize * 4;
                    row[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
            built = Some(sy);
        }
        let d = y * stride;
        dst[d..d + stride].copy_from_slice(row);
    }
}

/// Drop the alpha byte. The emulator's alpha is always 255.
pub fn rgba_to_rgb24(src: &[u8], dst: &mut Vec<u8>) {
    let n = src.len() / 4 * 3;
    if dst.len() != n {
        dst.resize(n, 0);
    }
    for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
        d.copy_from_slice(&s[..3]);
    }
}

/// RGBA → packed 4:2:2 `Y0 Cb Y1 Cr`, **BT.601 limited range** (Y 16–235,
/// C 16–240) — what a UVC webcam produces, so a consumer that assumes camera
/// conventions gets the levels it expects. Chroma for a pair is taken from the
/// pair's average colour.
///
/// An odd width pairs the last pixel with itself.
pub fn rgba_to_yuyv422(src: &[u8], w: u32, h: u32, dst: &mut Vec<u8>) {
    let pairs_per_row = w.div_ceil(2) as usize;
    let n = pairs_per_row * 4 * h as usize;
    if dst.len() != n {
        dst.resize(n, 0);
    }
    let sw = w as usize;
    for y in 0..h as usize {
        let s = y * sw * 4;
        let d = y * pairs_per_row * 4;
        for p in 0..pairs_per_row {
            let i0 = s + p * 8;
            let (r0, g0, b0) = (src[i0] as i32, src[i0 + 1] as i32, src[i0 + 2] as i32);
            let (r1, g1, b1) = if p * 2 + 1 < sw {
                (src[i0 + 4] as i32, src[i0 + 5] as i32, src[i0 + 6] as i32)
            } else {
                (r0, g0, b0)
            };
            let o = d + p * 4;
            dst[o] = luma(r0, g0, b0);
            dst[o + 2] = luma(r1, g1, b1);
            let (ar, ag, ab) = ((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2);
            dst[o + 1] = chroma_b(ar, ag, ab);
            dst[o + 3] = chroma_r(ar, ag, ab);
        }
    }
}

#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8
}

#[inline]
fn chroma_b(r: i32, g: i32, b: i32) -> u8 {
    (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8
}

#[inline]
fn chroma_r(r: i32, g: i32, b: i32) -> u8 {
    (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8
}

// ---------------------------------------------------------------------------
// v4l2loopback — detection and guidance, not an implementation
// ---------------------------------------------------------------------------

/// Virtual-camera support: **the emulator writes the camera itself**.
///
/// Run the game, press `v`, pick "Game Boy" in Chrome / Meet / Discord / OBS.
/// No bridge, no `ffmpeg`, no FIFO, no rule about which order to start things
/// in. That is the whole point, and everything below exists to make the one
/// keypress enough.
///
/// # What a V4L2 output node actually needs
///
/// Writing raw bytes at `/dev/videoN` and getting no error means **nothing** —
/// an un-negotiated write is accepted and produces nothing a consumer can
/// read. The device has to be told what the bytes are first:
///
/// 1. `VIDIOC_QUERYCAP`, to check the node really is a video *output*.
/// 2. `VIDIOC_S_FMT` with `V4L2_BUF_TYPE_VIDEO_OUTPUT` and
///    `V4L2_PIX_FMT_YUYV` at the wanted size. **The driver may change any of
///    it** and what comes back is authoritative — in particular
///    `bytesperline` (the row stride, which is *not* necessarily `width * 2`)
///    and `sizeimage` (how many bytes one `write` must carry).
/// 3. `write(2)` one frame at a time, in exactly that layout.
///
/// # Why raw `ioctl`s and no new dependency
///
/// The maintained V4L2 crates (`v4l`, `v4l2-sys-mit`) are permissively licensed
/// but link **libv4l2**, a *system* library. This repo has been bitten by
/// exactly that before — `cpal` pulling in ALSA made a fresh clone fail to
/// build until audio was feature-gated (see `CLAUDE.md`) — and the whole point
/// here is that a new user has *fewer* setup steps, not another `-devel`
/// package. So this speaks the kernel UAPI directly through the `libc`
/// dependency that is already here, the same way [`crate::shmframe`] speaks
/// POSIX shared memory.
///
/// Hand-writing UAPI structs is the risk that buys, and it is bounded by making
/// it a **compile error** to get one wrong: the `ioctl` request numbers encode
/// `sizeof` the struct they carry, so [`sink::VIDIOC_S_FMT`] is computed from
/// our own definition and then `const`-asserted against the value the kernel
/// headers publish. A layout mistake cannot reach a running program.
///
/// # Naming the camera after the game — what is actually possible
///
/// **It cannot be done from the emulator, and this is settled, not assumed.**
/// A v4l2loopback device's label is the `card_label` module parameter fixed
/// when the device is *created*; `/sys/class/video4linux/videoN/name` is
/// `-r--r--r--` and root-owned (checked on a live device), `modinfo` lists no
/// other route, and upstream's `v4l2loopback-ctl` has `add`/`delete`/`set-fps`/
/// `set-caps`/`set-timeout-image` and no rename. `VIDIOC_S_FMT` negotiates a
/// format, never an identity. Changing it means deleting and re-adding the
/// device as root, which drops every consumer — a one-off setup command we
/// document ([`install_hint_for`]), never a privilege escalation at `v` time.
/// So the honest answer is a *static* label chosen once, and
/// [`check_label`] tells the user when it does not match what they are
/// playing.
///
/// The `ffmpeg` bridge (`tools/virtual-camera.sh`) still exists and still
/// works — it is the route on a box with no v4l2loopback, and the only route
/// on macOS/Windows — but it is no longer the *supported* one.
pub mod v4l2 {
    use super::VideoSpec;
    use std::path::Path;

    /// The size a V4L2 node gets when the user has not asked for one.
    ///
    /// **16:9, and that is the whole point.** This used to be 640×480 on the
    /// reasoning that 4:3 is the size every webcam consumer accepts — which is
    /// true of *accepting* it and false of *showing* it. A video-call client
    /// lays its camera out in a 16:9 tile and fills that tile, so a 4:3 camera
    /// is centre-cropped on the way to the screen: 25 % of the height gone,
    /// 12.5 % off the top and 12.5 % off the bottom. On a face that is
    /// unremarkable. On a Game Boy screen it slices the dialogue box in half,
    /// which is exactly the report this default answers.
    ///
    /// Measured rather than assumed, because the mechanism is not the obvious
    /// one: Chrome hands the page the whole 640×480 frame (`getUserMedia`
    /// reports `resizeMode: "none"` against a v4l2loopback node — it does *not*
    /// crop the track), and the loss happens at the tile. Which means nothing
    /// on our side can detect it, and letterboxing correctly into a 4:3 frame
    /// does not help either: the consumer crops the bars *and* the picture with
    /// them. The only fix available to a camera is to be the shape the tile is.
    ///
    /// 1280×720 also happens to be the one size where the fit is perfect:
    /// 144 × 5 = 720, so the game fills the full height at an exact ×5 nearest
    /// magnification and only the pillarbox is bars. Measured cost of the
    /// larger frame: 1.83 ms/frame against 0.77 ms at 640×480 (`video_out
    /// --frames 3000`, YUYV), inside a 16.74 ms budget and paid only while a
    /// stream is live. See [`aspect_warning`] for what happens when a user
    /// pins something else.
    pub const DEFAULT_SIZE: (u32, u32) = (1280, 720);

    /// The aspect every video-call client crops to, as `(w, h)`.
    pub const TILE_ASPECT: (u32, u32) = (16, 9);

    /// A warning when a camera's configured geometry is not 16:9.
    ///
    /// Honouring an explicit `video_size` matters more than being right about
    /// it — forcing the geometry would be the emulator overruling a value the
    /// user typed. So the size stands and the consequence is named instead,
    /// with the number to change it to. Returns `None` for 16:9 (within a
    /// pixel, so 854×480 and friends pass).
    ///
    /// This is the *only* place the crop can be caught: by the time the frame
    /// is on the wire it is a correctly letterboxed 4:3 picture, and nothing on
    /// our side can tell that the consumer is about to cut the top and bottom
    /// off it.
    pub fn aspect_warning(w: u32, h: u32) -> Option<String> {
        if w == 0 || h == 0 {
            return None;
        }
        let (aw, ah) = TILE_ASPECT;
        // 16:9 to within one pixel of width at this height.
        let want = h as u64 * aw as u64 / ah as u64;
        if (w as i64 - want as i64).abs() <= 1 {
            return None;
        }
        let (dw, dh) = DEFAULT_SIZE;
        // First line stands alone: it is what a toast shows, and the callers
        // that only have one line of room already take `.lines().next()`.
        Some(format!(
            "video: {w}x{h} is not 16:9 — a video call will crop the top and \
             bottom off the game.\n\
             Meet, Zoom and Discord show a camera in a 16:9 tile and fill it. \
             The picture is letterboxed correctly into the frame and then the \
             app crops the frame, so nothing this end can rescue it. Use \
             `video_size = \"{dw}x{dh}\"` (the game lands at an exact x5) unless \
             the consumer shows the whole frame, like ffmpeg or OBS."
        ))
    }

    /// Is this path a V4L2 node? Writing raw frames at one silently fails
    /// (or errors obscurely) until its format is negotiated, so the caller
    /// refuses it and explains instead.
    pub fn is_video_device(p: &Path) -> bool {
        p.to_string_lossy().starts_with("/dev/video")
    }

    /// Is the loopback kernel module loaded?
    pub fn module_loaded() -> bool {
        Path::new("/sys/module/v4l2loopback").exists()
    }

    /// Is this node a **virtual** (loopback) device rather than a real camera?
    ///
    /// This distinction is load-bearing and not cosmetic. With
    /// `exclusive_caps=1` — the setting every guide recommends, because it is
    /// what makes browsers accept the device — a loopback node reports
    /// `V4L2_CAP_VIDEO_CAPTURE` **while another producer is feeding it**, and
    /// `V4L2_CAP_VIDEO_OUTPUT` only when it is free. So "this is a capture
    /// device" means two completely different things:
    ///
    /// * on a real webcam — permanent, and the user pointed at the wrong node;
    /// * on a loopback node — *someone else is already streaming to it*, which
    ///   clears on its own and must be retried, not given up on.
    ///
    /// Told apart through sysfs, because a v4l2loopback node is a virtual
    /// device (`/sys/devices/virtual/…`) and a USB or PCI camera never is.
    /// No `ioctl`, no root, no extra dependency.
    pub fn is_loopback(dev: &Path) -> bool {
        let Some(node) = dev.file_name() else {
            return false;
        };
        let sys = Path::new("/sys/class/video4linux").join(node);
        match std::fs::canonicalize(&sys) {
            Ok(real) => real.to_string_lossy().contains("/devices/virtual/"),
            Err(_) => false,
        }
    }

    /// The loopback nodes on this machine, for an error message that names the
    /// thing the user probably meant.
    pub fn loopback_devices() -> Vec<(String, String)> {
        devices()
            .into_iter()
            .filter(|(node, _)| is_loopback(Path::new(node)))
            .collect()
    }

    /// V4L2 devices present, as `(node, name)` — read from sysfs, so it needs
    /// no `ioctl` and no extra dependency.
    pub fn devices() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else {
            return out;
        };
        for e in dir.flatten() {
            let node = e.file_name().to_string_lossy().into_owned();
            if !node.starts_with("video") {
                continue;
            }
            let name = std::fs::read_to_string(e.path().join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();
            out.push((format!("/dev/{node}"), name));
        }
        out.sort();
        out
    }

    /// The label a V4L2 node advertises — the string OBS, Zoom and
    /// `getUserMedia` show in their camera list. Read from sysfs.
    pub fn device_label(dev: &Path) -> Option<String> {
        let node = dev.file_name()?.to_string_lossy().into_owned();
        let name = std::fs::read_to_string(
            Path::new("/sys/class/video4linux").join(&node).join("name"),
        )
        .ok()?;
        Some(name.trim().to_string())
    }

    /// Does a device label plausibly name this game?
    ///
    /// Deliberately loose: labels get suffixed and re-cased by whoever created
    /// them (`"Game Boy: POKEMON BLUE"`), so it is a containment test on
    /// alphanumerics only. The point is to catch "you are streaming Tetris
    /// through a camera called Pokemon Blue", not to police formatting.
    pub fn label_matches(label: &str, title: &str) -> bool {
        let squash = |s: &str| -> String {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect()
        };
        let (l, t) = (squash(label), squash(title));
        !t.is_empty() && (l.contains(&t) || t.contains(&l))
    }

    /// Check the loopback device the user says they are feeding, and complain
    /// if its label names a different game.
    ///
    /// **A v4l2loopback device cannot be renamed while it exists.** The label
    /// comes from the `card_label` module parameter at load time, or from
    /// `v4l2loopback-ctl add -n`; the upstream README documents no rename, and
    /// the only way to change it is to delete and re-create the device — which
    /// needs root and drops every consumer attached to it. So the honest thing
    /// is to *verify* and hand over the exact command, not to pretend we can
    /// fix it.
    pub fn check_label(dev: &Path, title: &str) -> Option<String> {
        let label = device_label(dev)?;
        let title = super::clean_title(title);
        if label_matches(&label, &title) {
            return None;
        }
        Some(format!(
            "video: {} is labelled \"{label}\" but you are playing \"{title}\" — a \
             v4l2loopback device cannot be renamed while it exists, so re-create it \
             (needs root, drops consumers):\n  \
             sudo v4l2loopback-ctl delete {}\n  \
             sudo v4l2loopback-ctl add -n \"{title}\" {}",
            dev.display(),
            dev.display(),
            dev.display(),
        ))
    }

    /// The exact commands to install and create a loopback device. Printed
    /// verbatim when someone points `video_out` at a V4L2 node.
    pub fn install_hint() -> String {
        install_hint_for("Game Boy")
    }

    /// What to do when there is no virtual camera on this machine at all.
    ///
    /// This is now the **only** setup step between a fresh install and a
    /// working camera, so it has to be something a person can paste, and it has
    /// to survive a reboot — a device created by hand with `modprobe`
    /// disappears on the next one, which reads as "it broke again".
    pub fn install_hint_for_missing() -> String {
        String::from(
            "there is no virtual camera on this machine yet. One-off setup, needs \
             root once:\n  \
             Fedora:  sudo dnf install akmod-v4l2loopback v4l2loopback-utils\n  \
             Debian:  sudo apt install v4l2loopback-dkms v4l-utils\n  \
             Arch:    sudo pacman -S v4l2loopback-dkms v4l-utils\n\
             then make one that survives a reboot:\n  \
             echo v4l2loopback | sudo tee /etc/modules-load.d/v4l2loopback.conf\n  \
             echo 'options v4l2loopback devices=1 video_nr=10 card_label=\"Game Boy\" \
             exclusive_caps=1' | sudo tee /etc/modprobe.d/v4l2loopback.conf\n  \
             sudo modprobe -r v4l2loopback; sudo modprobe v4l2loopback\n\
             then set `video_out = \"/dev/video10\"` and press v. \
             The camera's NAME is fixed when the device is created (card_label above) \
             — it cannot be changed while the device exists, so pick one you like.",
        )
    }

    /// [`install_hint`], with the camera named after the game — because
    /// "POKEMON BLUE" in a camera picker is worth far more than "Dummy video
    /// device (0x0000)", and the label can only be chosen **when the device is
    /// created**.
    pub fn install_hint_for(title: &str) -> String {
        let label = super::clean_title(title);
        let label = if label.is_empty() {
            "Game Boy".to_string()
        } else {
            label
        };
        format!(
            "install v4l2loopback (needs root, once):\n  \
             Fedora:  sudo dnf install akmod-v4l2loopback v4l2loopback-utils\n  \
             Debian:  sudo apt install v4l2loopback-dkms v4l-utils\n  \
             Arch:    sudo pacman -S v4l2loopback-dkms v4l-utils\n\
             then create the camera, named after the game:\n  \
             sudo modprobe v4l2loopback video_nr=10 card_label=\"{label}\" exclusive_caps=1\n  \
             (or, with the module already loaded: sudo v4l2loopback-ctl add -n \"{label}\" /dev/video10)"
        )
    }

    // -----------------------------------------------------------------------
    // Native output: the kernel UAPI, spoken directly
    // -----------------------------------------------------------------------

    /// `VIDIOC_QUERYCAP` / `VIDIOC_S_FMT` / `write(2)` against a V4L2 output
    /// node. Linux + `libc` only; every other target keeps the old refusal.
    #[cfg(all(target_os = "linux", feature = "libc"))]
    pub mod sink {
        use std::ffi::CString;
        use std::path::{Path, PathBuf};
        use std::time::Instant;

        /// `_IOC(dir, type, nr, size)` — the kernel's `ioctl` request encoding.
        ///
        /// Computed rather than pasted **on purpose**: `size` is `sizeof` the
        /// struct the call carries, so a request number derived from our own
        /// definitions and then checked against the published constant proves
        /// the definitions match the kernel's. That is the whole safety net for
        /// hand-written UAPI structs, and it is a compile-time one.
        const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> libc::c_ulong {
            ((dir << 30) | ((size as u32) << 16) | (ty << 8) | nr) as libc::c_ulong
        }
        const READ: u32 = 2;
        const WRITE: u32 = 1;
        const V: u32 = b'V' as u32;

        /// `struct v4l2_capability` (linux/videodev2.h).
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct Capability {
            pub driver: [u8; 16],
            pub card: [u8; 32],
            pub bus_info: [u8; 32],
            pub version: u32,
            pub capabilities: u32,
            pub device_caps: u32,
            pub reserved: [u32; 3],
        }

        /// `struct v4l2_pix_format`.
        #[repr(C)]
        #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
        pub struct PixFormat {
            pub width: u32,
            pub height: u32,
            pub pixelformat: u32,
            pub field: u32,
            pub bytesperline: u32,
            pub sizeimage: u32,
            pub colorspace: u32,
            pub private: u32,
            pub flags: u32,
            /// `ycbcr_enc` / `hsv_enc`, a union in the header.
            pub enc: u32,
            pub quantization: u32,
            pub xfer_func: u32,
        }

        /// `struct v4l2_format`. The payload is a 200-byte union aligned to 8,
        /// of which `pix` is the first member.
        #[repr(C, align(8))]
        #[derive(Clone, Copy)]
        pub struct Format {
            pub kind: u32,
            _pad: u32,
            pub pix: PixFormat,
            _rest: [u8; 200 - core::mem::size_of::<PixFormat>()],
        }

        pub const VIDIOC_QUERYCAP: libc::c_ulong =
            ioc(READ, V, 0, core::mem::size_of::<Capability>());
        pub const VIDIOC_S_FMT: libc::c_ulong =
            ioc(READ | WRITE, V, 5, core::mem::size_of::<Format>());

        // If any struct above is laid out differently from the kernel's, these
        // request numbers change and the build stops here — long before a wrong
        // `ioctl` can be issued against a real device.
        const _: () = assert!(core::mem::size_of::<Capability>() == 104);
        const _: () = assert!(core::mem::size_of::<PixFormat>() == 48);
        const _: () = assert!(core::mem::size_of::<Format>() == 208);
        const _: () = assert!(VIDIOC_QUERYCAP == 0x8068_5600);
        const _: () = assert!(VIDIOC_S_FMT == 0xc0d0_5605);

        pub const BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
        pub const CAP_VIDEO_OUTPUT: u32 = 0x0000_0002;
        pub const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
        pub const FIELD_NONE: u32 = 1;
        /// BT.601, which is what [`super::super::rgba_to_yuyv422`] emits.
        pub const COLORSPACE_SMPTE170M: u32 = 1;
        /// `V4L2_PIX_FMT_YUYV`, fourcc `'Y' 'U' 'Y' 'V'`.
        pub const PIX_FMT_YUYV: u32 = fourcc(b"YUYV");

        pub const fn fourcc(c: &[u8; 4]) -> u32 {
            (c[0] as u32)
                | ((c[1] as u32) << 8)
                | ((c[2] as u32) << 16)
                | ((c[3] as u32) << 24)
        }

        /// What the driver agreed to. Authoritative — the numbers we *asked*
        /// for are not what the frames must be written as.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Negotiated {
            pub width: u32,
            pub height: u32,
            /// Row stride in bytes. **Not** necessarily `width * 2`.
            pub bytesperline: u32,
            /// Exact byte count of one `write`.
            pub sizeimage: u32,
        }

        impl Negotiated {
            /// Bytes of real pixel data per row, before any stride padding.
            pub fn row_bytes(&self) -> usize {
                self.width as usize * 2
            }
        }

        /// An owned file descriptor. Closed on drop — which is fine here,
        /// because this is a resource with no persistence semantics, unlike the
        /// battery saves and shm objects `Drop` must not be trusted with.
        struct Fd(libc::c_int);

        impl Drop for Fd {
            fn drop(&mut self) {
                unsafe { libc::close(self.0) };
            }
        }

        /// Why a device could not be used, in words a user can act on.
        pub enum OpenError {
            /// No such node. Almost always "v4l2loopback is not installed".
            Missing(String),
            /// It exists but is not usable right now (busy, permissions).
            Unavailable(String),
            /// It exists and is not an output device (a real webcam).
            NotAnOutput(String),
        }

        impl OpenError {
            pub fn message(&self) -> &str {
                match self {
                    OpenError::Missing(s)
                    | OpenError::Unavailable(s)
                    | OpenError::NotAnOutput(s) => s,
                }
            }
            /// Is this worth retrying *soon*? A busy device frees up in
            /// seconds; a missing one waits for a `modprobe`, so it is retried
            /// far more slowly rather than not at all.
            pub fn transient(&self) -> bool {
                matches!(self, OpenError::Unavailable(_))
            }

            /// Can this ever clear without the user changing the config?
            ///
            /// A real webcam will never become writable however long we wait,
            /// so promising to "keep retrying" would be a lie.
            pub fn recoverable(&self) -> bool {
                !matches!(self, OpenError::NotAnOutput(_))
            }
        }

        /// Open `path` as a video output and negotiate `w`×`h` YUYV.
        ///
        /// Never blocks: the node is opened `O_NONBLOCK`, and the two `ioctl`s
        /// are bounded driver calls.
        fn negotiate(path: &Path, w: u32, h: u32) -> Result<(Fd, Negotiated), OpenError> {
            let c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| {
                OpenError::Missing(format!("{}: not a path", path.display()))
            })?;
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
            if fd < 0 {
                let e = std::io::Error::last_os_error();
                return Err(match e.raw_os_error() {
                    Some(libc::ENOENT) | Some(libc::ENODEV) => {
                        OpenError::Missing(format!("{} does not exist", path.display()))
                    }
                    _ => OpenError::Unavailable(format!("{}: {e}", path.display())),
                });
            }
            let fd = Fd(fd);

            // 1. Is it an output at all? A real webcam is a capture device and
            //    writing at it can only ever fail; say which it is rather than
            //    letting S_FMT return a bare EINVAL.
            let mut cap: Capability = unsafe { core::mem::zeroed() };
            if unsafe { libc::ioctl(fd.0, VIDIOC_QUERYCAP, &mut cap) } < 0 {
                let e = std::io::Error::last_os_error();
                return Err(OpenError::NotAnOutput(format!(
                    "{} is not a V4L2 device ({e})",
                    path.display()
                )));
            }
            let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 {
                cap.device_caps
            } else {
                cap.capabilities
            };
            if caps & CAP_VIDEO_OUTPUT == 0 {
                // A loopback node reports CAPTURE precisely *because* something
                // else is already feeding it (`exclusive_caps=1`). That is a
                // transient condition, not the wrong device — see
                // [`super::is_loopback`].
                return Err(if super::is_loopback(path) {
                    OpenError::Unavailable(format!(
                        "{} (\"{}\") is already being fed by another program — \
                         a camera has one producer at a time",
                        path.display(),
                        cstr(&cap.card),
                    ))
                } else {
                    OpenError::NotAnOutput(format!(
                        "{} is \"{}\" — a real camera, which can only be read from. \
                         Point video_out at a v4l2loopback node instead{}",
                        path.display(),
                        cstr(&cap.card),
                        match super::loopback_devices().first() {
                            Some((n, l)) => format!(" (this machine has {n}, \"{l}\")"),
                            None => String::new(),
                        },
                    ))
                });
            }

            // 2. Negotiate. What comes back is what the frames must be.
            let mut fmt: Format = unsafe { core::mem::zeroed() };
            fmt.kind = BUF_TYPE_VIDEO_OUTPUT;
            fmt.pix = PixFormat {
                width: w,
                height: h,
                pixelformat: PIX_FMT_YUYV,
                field: FIELD_NONE,
                colorspace: COLORSPACE_SMPTE170M,
                // Leave stride and size at 0: this is the driver's to fill in,
                // and passing a guess is how a wrong stride gets accepted.
                ..PixFormat::default()
            };
            if unsafe { libc::ioctl(fd.0, VIDIOC_S_FMT, &mut fmt) } < 0 {
                let e = std::io::Error::last_os_error();
                return Err(OpenError::Unavailable(format!(
                    "{}: the driver would not accept {w}x{h} YUYV ({e})",
                    path.display()
                )));
            }
            if fmt.pix.pixelformat != PIX_FMT_YUYV {
                return Err(OpenError::Unavailable(format!(
                    "{} answered with pixel format {} instead of YUYV",
                    path.display(),
                    fourcc_name(fmt.pix.pixelformat),
                )));
            }
            let mut n = Negotiated {
                width: fmt.pix.width,
                height: fmt.pix.height,
                bytesperline: fmt.pix.bytesperline,
                sizeimage: fmt.pix.sizeimage,
            };
            // A driver is allowed to leave these at 0; fall back to the packed
            // layout rather than writing a zero-length frame forever.
            if n.bytesperline == 0 {
                n.bytesperline = n.width * 2;
            }
            if n.sizeimage == 0 {
                n.sizeimage = n.bytesperline * n.height;
            }
            Ok((fd, n))
        }

        /// A NUL-padded fixed-size C string as a `String`.
        fn cstr(b: &[u8]) -> String {
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).into_owned()
        }

        /// A fourcc back as its four characters, for an error message.
        pub fn fourcc_name(v: u32) -> String {
            (0..4)
                .map(|i| ((v >> (i * 8)) & 0xFF) as u8 as char)
                .collect()
        }

        /// The camera, from the emulator's side.
        ///
        /// Holds the same three promises as every other sink here: never blocks
        /// startup, survives the device coming and going, and never turns a
        /// consumer's speed into frame time.
        pub struct V4l2Sink {
            path: PathBuf,
            fd: Option<Fd>,
            format: Option<Negotiated>,
            retry_at: Option<Instant>,
            /// Last reason the device could not be opened, for the status row.
            pub last_error: Option<String>,
            /// …and whether that reason can clear on its own. A busy device
            /// will; a real webcam will not, and telling the user we are
            /// "retrying" something that can never succeed is noise.
            pub last_error_transient: bool,
            /// Scratch: the frame re-laid-out at the negotiated stride. Only
            /// used when `bytesperline != width * 2`.
            strided: Vec<u8>,
        }

        /// What one attempt to put a frame on the device achieved.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Put {
            Wrote,
            /// Not attached; nothing was lost.
            NoDevice,
            /// The driver would not take it right now — frame dropped.
            Busy,
            /// The device went away.
            Lost,
            /// Our frame is not the size the driver negotiated. A bug, not a
            /// condition — reported rather than written as garbage.
            WrongSize,
        }

        impl V4l2Sink {
            pub fn new(path: PathBuf) -> V4l2Sink {
                V4l2Sink {
                    path,
                    fd: None,
                    format: None,
                    retry_at: None,
                    last_error: None,
                    last_error_transient: true,
                    strided: Vec::new(),
                }
            }

            pub fn live(&self) -> bool {
                self.fd.is_some()
            }

            pub fn format(&self) -> Option<Negotiated> {
                self.format
            }

            pub fn path(&self) -> &Path {
                &self.path
            }

            /// Attempt to (re)attach, at most once per `every`.
            ///
            /// `want` is the size asked for; what comes back may differ and is
            /// what the caller must then produce.
            pub fn attach(
                &mut self,
                want: (u32, u32),
                now: Instant,
                every: std::time::Duration,
            ) {
                if self.fd.is_some() {
                    return;
                }
                if let Some(t) = self.retry_at {
                    if now < t {
                        return;
                    }
                }
                match negotiate(&self.path, want.0, want.1) {
                    Ok((fd, n)) => {
                        self.fd = Some(fd);
                        self.format = Some(n);
                        self.last_error = None;
                        self.last_error_transient = true;
                        self.retry_at = None;
                    }
                    Err(e) => {
                        // A missing device will not appear four times a second;
                        // a busy one will free up.
                        let back = if e.transient() { every } else { every * 20 };
                        self.last_error = Some(e.message().to_string());
                        self.last_error_transient = e.recoverable();
                        self.retry_at = Some(now + back);
                    }
                }
            }

            pub fn detach(&mut self) {
                self.fd = None;
                self.format = None;
                self.retry_at = None;
            }

            /// Write one frame of packed YUYV (`width * 2` bytes per row, no
            /// padding). Re-strides it if the driver asked for padding.
            pub fn put(&mut self, packed: &[u8]) -> Put {
                let (Some(fd), Some(n)) = (self.fd.as_ref(), self.format) else {
                    return Put::NoDevice;
                };
                let row = n.row_bytes();
                if packed.len() != row * n.height as usize {
                    return Put::WrongSize;
                }
                let buf: &[u8] = if n.bytesperline as usize == row
                    && n.sizeimage as usize == packed.len()
                {
                    packed
                } else {
                    // The driver wants padded rows, or a longer buffer than the
                    // pixels occupy. Both are legal and both corrupt the picture
                    // if ignored — this is why `bytesperline` is read back at
                    // all rather than assumed to be `width * 2`.
                    let stride = n.bytesperline as usize;
                    let total = (n.sizeimage as usize).max(stride * n.height as usize);
                    if self.strided.len() != total {
                        self.strided.resize(total, 0);
                    }
                    for y in 0..n.height as usize {
                        let s = y * row;
                        let d = y * stride;
                        self.strided[d..d + row].copy_from_slice(&packed[s..s + row]);
                    }
                    &self.strided
                };
                let wrote = unsafe {
                    libc::write(fd.0, buf.as_ptr() as *const libc::c_void, buf.len())
                };
                if wrote >= 0 {
                    return Put::Wrote;
                }
                let e = std::io::Error::last_os_error();
                match e.raw_os_error() {
                    // The driver's buffers are full: the consumer is not
                    // keeping up. Drop the frame — never wait for it.
                    Some(libc::EAGAIN) | Some(libc::EBUSY) => Put::Busy,
                    Some(libc::EINTR) => Put::Busy,
                    _ => {
                        self.detach();
                        Put::Lost
                    }
                }
            }
        }
    }

    /// Native V4L2 output is a Linux-with-`libc` capability.
    pub const NATIVE_OUTPUT: bool = cfg!(all(target_os = "linux", feature = "libc"));

    /// Why we will not write straight at `/dev/videoN`, and what to do instead.
    ///
    /// Only reached on a build with no native V4L2 output (not Linux, or no
    /// `libc` feature). On Linux the device is driven directly — see [`sink`].
    pub fn direct_write_refusal(p: &Path, spec: &VideoSpec) -> String {
        let (w, h) = spec.size.unwrap_or((super::SRC_W, super::SRC_H));
        let fifo = "/tmp/gb.raw";
        let mut s = format!(
            "video: {} is a V4L2 device — raw writes to one need a negotiated \
             format (VIDIOC_S_FMT), which this emulator deliberately does not do. \
             Feed it through ffmpeg instead (one extra process, and you get format \
             conversion for free):\n  \
             mkfifo {fifo}\n  \
             ffmpeg -f rawvideo -pix_fmt {} -s {w}x{h} -framerate {} -i {fifo} \
             -f v4l2 -pix_fmt yuyv422 {}\n  \
             GB_VIDEO_OUT={fifo} GB_VIDEO_FORMAT=yuyv GB_VIDEO_SIZE={w}x{h} <run the emulator>\n\
             or just: tools/virtual-camera.sh",
            p.display(),
            spec.format.ffmpeg_name(),
            super::DMG_HZ,
            p.display(),
        );
        if !module_loaded() {
            s.push_str("\n(the v4l2loopback module is not loaded here — ");
            s.push_str(&install_hint());
            s.push(')');
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(fmt: PixelFormat, pace: Pace, size: Option<(u32, u32)>) -> VideoSpec {
        VideoSpec {
            dest: Destination::Stdout,
            format: fmt,
            source: Source::Native,
            size,
            pace,
            title: None,
        }
    }

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        c.iter()
            .cycle()
            .take(w as usize * h as usize * 4)
            .copied()
            .collect()
    }

    #[test]
    fn destination_parses_and_disables() {
        assert_eq!(Destination::parse("-"), Some(Destination::Stdout));
        assert_eq!(Destination::parse("StdOut"), Some(Destination::Stdout));
        assert_eq!(
            Destination::parse("/tmp/gb.raw"),
            Some(Destination::Path(PathBuf::from("/tmp/gb.raw")))
        );
        for off in ["", "  ", "off", "OFF", "false", "0"] {
            assert_eq!(Destination::parse(off), None, "{off:?}");
        }
    }

    #[test]
    fn sizes_and_formats_parse() {
        assert_eq!(parse_size("640x480"), Some((640, 480)));
        assert_eq!(parse_size(" 320 X 288 "), Some((320, 288)));
        assert_eq!(parse_size("640"), None);
        assert_eq!(parse_size("0x480"), None);
        assert_eq!(PixelFormat::parse("YUYV"), Some(PixelFormat::Yuyv422));
        assert_eq!(PixelFormat::parse("bgr"), None);
        assert_eq!(Pace::parse("emu"), Some(Pace::Emulated));
        assert_eq!(Source::parse("display"), Some(Source::Display));
    }

    #[test]
    fn cartridge_titles_are_cleaned_for_humans_and_for_filenames() {
        // Header titles are padded with NULs and may hold junk.
        assert_eq!(clean_title("POKEMON BLUE\0\0\0\0"), "POKEMON BLUE");
        assert_eq!(clean_title("  TETRIS   \0"), "TETRIS");
        assert_eq!(clean_title("ZELDA\u{7}\u{1}"), "ZELDA");
        assert_eq!(clean_title("SUPER   MARIO"), "SUPER MARIO");
        assert_eq!(clean_title("\0\0\0"), "");

        assert_eq!(file_safe_title("POKEMON BLUE"), "POKEMON_BLUE");
        assert_eq!(file_safe_title("../../etc/passwd"), "etc_passwd");
        assert_eq!(file_safe_title("\0\0"), "rom");
        assert!(!file_safe_title("a/b").contains('/'));
    }

    #[test]
    fn a_recording_is_named_after_the_game_and_carries_its_own_ffmpeg_args() {
        let n = recording_name(
            "POKEMON BLUE\0",
            "20260813-142530",
            640,
            480,
            PixelFormat::Yuyv422,
        );
        assert_eq!(n, "POKEMON_BLUE-20260813-142530-640x480-yuyv422.raw");
        // Everything `-f rawvideo` needs is in the name.
        assert!(n.contains("640x480") && n.contains("yuyv422"));
    }

    #[test]
    fn placeholders_name_the_stream_after_the_game() {
        assert_eq!(
            expand_placeholders("/tmp/%t-%s.raw", "SUPER MARIO", "20260813-142530"),
            "/tmp/SUPER_MARIO-20260813-142530.raw"
        );
        assert_eq!(expand_placeholders("/tmp/gb.raw", "X", "Y"), "/tmp/gb.raw");
    }

    #[test]
    fn a_camera_label_is_matched_loosely_but_a_different_game_is_caught() {
        assert!(v4l2::label_matches("POKEMON BLUE", "POKEMON BLUE"));
        assert!(v4l2::label_matches(
            "Game Boy: Pokemon Blue",
            "POKEMON BLUE"
        ));
        assert!(v4l2::label_matches("pokemonblue", "POKEMON BLUE"));
        assert!(!v4l2::label_matches(
            "Dummy video device (0x0000)",
            "TETRIS"
        ));
        assert!(!v4l2::label_matches("POKEMON BLUE", "TETRIS"));
        assert!(!v4l2::label_matches("anything", ""));
    }

    /// Whatever V4L2 devices this box has (possibly none), the sysfs label
    /// reader agrees with the enumeration. Passes on a machine with no camera
    /// at all, which is the CI case.
    #[test]
    fn v4l2_labels_are_read_from_sysfs_when_a_device_exists() {
        for (node, name) in v4l2::devices() {
            assert_eq!(
                v4l2::device_label(std::path::Path::new(&node)).as_deref(),
                Some(name.as_str()),
                "{node}"
            );
        }
    }

    #[test]
    fn the_camera_hint_names_the_device_after_the_game() {
        let h = v4l2::install_hint_for("POKEMON BLUE\0\0");
        assert!(h.contains("card_label=\"POKEMON BLUE\""), "{h}");
        assert!(
            h.contains("v4l2loopback-ctl add -n \"POKEMON BLUE\""),
            "{h}"
        );
    }

    #[test]
    fn cli_flags_split_out_of_the_argument_list() {
        let args: Vec<String> = ["game.gb", "--video-out", "-", "--video-format=yuyv"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (found, rest) = split_cli_flags(args).unwrap();
        assert_eq!(
            found,
            vec![
                ("GB_VIDEO_OUT", "-".to_string()),
                ("GB_VIDEO_FORMAT", "yuyv".to_string()),
            ]
        );
        assert_eq!(rest, vec!["game.gb".to_string()]);
    }

    #[test]
    fn a_flag_without_a_value_is_an_error_not_a_silent_default() {
        let args: Vec<String> = vec!["--video-out".to_string()];
        assert_eq!(
            split_cli_flags(args).unwrap_err(),
            "--video-out needs a value"
        );
    }

    #[test]
    fn disabled_emitter_writes_nothing() {
        let mut v = VideoOut::disabled();
        assert!(!v.is_enabled());
        v.frame(&solid(160, 144, [1, 2, 3, 255]), 160, 144, Instant::now());
        assert!(v.captured().is_empty());
        assert_eq!(v.stats().frames, 0);
    }

    #[test]
    fn emulated_pace_writes_exactly_one_frame_per_emulated_frame() {
        let mut v = VideoOut::in_memory(spec(PixelFormat::Rgba, Pace::Emulated, None));
        let f = solid(160, 144, [10, 20, 30, 255]);
        let t = Instant::now();
        for _ in 0..5 {
            v.frame(&f, 160, 144, t); // same instant: pacing must not matter
        }
        assert_eq!(v.stats().frames, 5);
        assert_eq!(v.captured().len(), 5 * 160 * 144 * 4);
        assert_eq!(v.stats().duplicated, 0);
        assert_eq!(v.stats().dropped, 0);
    }

    #[test]
    fn realtime_pace_drops_fast_forwarded_frames() {
        let mut v = VideoOut::in_memory(spec(PixelFormat::Rgba, Pace::Realtime, None));
        let f = solid(160, 144, [1, 1, 1, 255]);
        let t0 = Instant::now();
        // Four emulated frames inside one frame period == 4x fast-forward.
        for i in 0..4 {
            v.frame(&f, 160, 144, t0 + Duration::from_micros(i * 1_000));
        }
        assert_eq!(v.stats().frames, 1, "only the first is due");
        assert_eq!(v.stats().dropped, 3);
        // A second later the stream has advanced by wall clock, not by
        // emulated frames.
        v.frame(&f, 160, 144, t0 + FRAME_PERIOD);
        assert_eq!(v.stats().frames, 2);
    }

    #[test]
    fn realtime_pace_is_one_to_one_at_normal_speed() {
        let mut v = VideoOut::in_memory(spec(PixelFormat::Rgba, Pace::Realtime, None));
        let f = solid(160, 144, [1, 1, 1, 255]);
        let t0 = Instant::now();
        for i in 0..600u32 {
            // Jitter of ±1 ms, which is more than this box actually shows.
            let jitter = Duration::from_micros((i as u64 % 3) * 500);
            v.frame(&f, 160, 144, t0 + FRAME_PERIOD * i + jitter);
        }
        let s = v.stats();
        assert_eq!(s.frames, 600, "one output frame per emulated frame");
        assert_eq!(s.duplicated, 0);
        assert_eq!(s.dropped, 0);
        assert_eq!(s.resyncs, 0);
    }

    #[test]
    fn a_paused_emulator_still_produces_a_steady_stream() {
        let mut v = VideoOut::in_memory(spec(PixelFormat::Rgba, Pace::Realtime, None));
        let f = solid(160, 144, [7, 7, 7, 255]);
        let t0 = Instant::now();
        v.frame(&f, 160, 144, t0);
        // Pause: the loop only calls idle(), every 4 ms.
        for i in 1..=250u32 {
            v.idle(t0 + Duration::from_millis(4) * i);
        }
        // 1 s of pause == ~60 frames of held picture, not a stalled pipe.
        let s = v.stats();
        assert!(
            (58..=62).contains(&s.frames),
            "expected ~60 frames over 1 s, got {}",
            s.frames
        );
        assert_eq!(s.frames - 1, s.duplicated);
    }

    #[test]
    fn a_long_stall_resyncs_instead_of_dumping_a_burst() {
        let mut v = VideoOut::in_memory(spec(PixelFormat::Rgba, Pace::Realtime, None));
        let f = solid(160, 144, [3, 3, 3, 255]);
        let t0 = Instant::now();
        v.frame(&f, 160, 144, t0);
        v.frame(&f, 160, 144, t0 + Duration::from_secs(10));
        assert_eq!(v.stats().resyncs, 1);
        assert!(
            v.stats().frames <= 1 + MAX_BURST as u64,
            "burst must be bounded, got {}",
            v.stats().frames
        );
    }

    #[test]
    fn rgb24_drops_alpha_only() {
        let mut out = Vec::new();
        rgba_to_rgb24(&[1, 2, 3, 255, 4, 5, 6, 255], &mut out);
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn yuyv_is_limited_range_bt601() {
        let mut out = Vec::new();
        // Pure black and pure white, one pair each.
        rgba_to_yuyv422(&[0, 0, 0, 255, 0, 0, 0, 255], 2, 1, &mut out);
        assert_eq!(out, vec![16, 128, 16, 128], "black is Y=16, neutral chroma");
        rgba_to_yuyv422(&[255, 255, 255, 255, 255, 255, 255, 255], 2, 1, &mut out);
        assert_eq!(out[0], 235, "white is Y=235");
        assert_eq!(out[2], 235);
        assert!((out[1] as i32 - 128).abs() <= 1);
        assert!((out[3] as i32 - 128).abs() <= 1);
        // Saturated red: Cr high, Cb low.
        rgba_to_yuyv422(&[255, 0, 0, 255, 255, 0, 0, 255], 2, 1, &mut out);
        assert!(out[3] > 200 && out[1] < 100, "red: {out:?}");
    }

    #[test]
    fn yuyv_frame_is_exactly_two_bytes_per_pixel() {
        let mut out = Vec::new();
        rgba_to_yuyv422(&solid(640, 480, [12, 34, 56, 255]), 640, 480, &mut out);
        assert_eq!(out.len(), 640 * 480 * 2);
    }

    #[test]
    fn fit_prefers_integer_scaling_and_letterboxes_the_rest() {
        // 160x144 in 640x480: 4x wide, 3.33x tall -> 3x = 480x432, bars all round.
        assert_eq!(fit_size(160, 144, 640, 480), (480, 432));
        // 16:9 1280x720 -> 5x = 800x720, pillarboxed, exactly full height.
        assert_eq!(fit_size(160, 144, 1280, 720), (800, 720));
        // Exact multiple: no bars.
        assert_eq!(fit_size(160, 144, 320, 288), (320, 288));
        // Smaller than the source: aspect-preserving fractional fit.
        assert_eq!(fit_size(160, 144, 80, 80), (80, 72));
    }

    #[test]
    fn letterbox_centres_the_picture_and_blacks_the_bars() {
        let src = solid(2, 2, [200, 100, 50, 255]);
        let (xm, ym) = fit_maps(2, 2, 6, 4);
        let (mut dst, mut row) = (Vec::new(), Vec::new());
        letterbox(&src, 2, &mut dst, &mut row, 6, 4, &xm, &ym);
        assert_eq!(dst.len(), 6 * 4 * 4);
        let px = |x: usize, y: usize| &dst[(y * 6 + x) * 4..(y * 6 + x) * 4 + 4];
        // 2x2 scaled 2x = 4x4, centred at x=1..5, y=0..4.
        assert_eq!(px(0, 0), &[0, 0, 0, 255], "left bar is opaque black");
        assert_eq!(px(5, 3), &[0, 0, 0, 255], "right bar is opaque black");
        assert_eq!(px(2, 1), &[200, 100, 50, 255], "picture is intact");
    }

    /// The bounding box of everything that is not an opaque-black bar, as
    /// `(x0, y0, x1, y1)` half-open — i.e. where the picture actually landed.
    fn content_box(f: &[u8], w: u32, h: u32) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
        for y in 0..h {
            for x in 0..w {
                let p = ((y * w + x) * 4) as usize;
                if f[p..p + 3] != [0, 0, 0] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        (x0, y0, x1, y1)
    }

    /// The whole game picture must survive every geometry, which is the one
    /// promise a camera feed makes. Swept rather than spot-checked because the
    /// reported failure was at a size nobody had tried.
    #[test]
    fn fit_never_crops_at_any_size_or_aspect() {
        let sources = [(160u32, 144u32), (320, 288), (640, 576), (480, 432)];
        let dests = [
            (640, 480),   // 4:3
            (1280, 720),  // 16:9
            (1920, 1080), // 16:9, larger
            (854, 480),   // 16:9, odd width
            (641, 481),   // odd both ways
            (100, 100),   // square, smaller than the source
            (17, 300),    // absurdly tall
            (300, 17),    // absurdly wide
            (1, 1),       // degenerate
            (160, 144),   // exact
        ];
        for (sw, sh) in sources {
            for (dw, dh) in dests {
                let (tw, th) = fit_size(sw, sh, dw, dh);
                assert!(
                    tw <= dw && th <= dh,
                    "{sw}x{sh} into {dw}x{dh} produced {tw}x{th} — larger than \
                     the frame is a crop"
                );
                assert!(tw > 0 && th > 0, "{sw}x{sh} into {dw}x{dh} vanished");
                // Aspect is preserved to within the rounding of one pixel on
                // the shorter axis; never stretched to fill.
                // |th - tw*sh/sw| <= 1, rearranged to stay in integers.
                let err = (tw as i64 * sh as i64 - th as i64 * sw as i64).abs();
                assert!(
                    err <= sw.max(sh) as i64,
                    "{sw}x{sh} into {dw}x{dh} -> {tw}x{th} is stretched"
                );
                // Centred, with the odd leftover pixel never more than one.
                let (ox, oy) = fit_origin(tw, th, dw, dh);
                assert!(
                    (dw - tw - ox) as i64 - ox as i64 <= 1
                        && (dh - th - oy) as i64 - oy as i64 <= 1,
                    "{sw}x{sh} into {dw}x{dh} is off-centre by more than a pixel"
                );
                // And every source row and column is represented somewhere,
                // i.e. nothing was dropped off an edge.
                let (xm, ym) = fit_maps(sw, sh, dw, dh);
                let seen = |m: &[u32]| -> (u32, u32) {
                    let live: Vec<u32> =
                        m.iter().copied().filter(|&v| v != u32::MAX).collect();
                    (*live.first().unwrap(), *live.last().unwrap())
                };
                // Point sampling drops source pixels when it shrinks — that is
                // what nearest-neighbour is. What it must never do is stop
                // short of an edge: the first and last samples have to fall in
                // the first and last *band* of source pixels one output pixel
                // covers. At 1:1 or magnified a band is one pixel, so this is
                // exactly "column 0 and column sw-1 are shown".
                let band = |s: u32, t: u32| s.div_ceil(t);
                let (bx, by) = (band(sw, tw), band(sh, th));
                let (fx, lx) = seen(&xm);
                let (fy, ly) = seen(&ym);
                assert!(fx < bx, "{sw}x{sh} into {dw}x{dh}: left edge lost ({fx})");
                assert!(fy < by, "{sw}x{sh} into {dw}x{dh}: top edge lost ({fy})");
                assert!(
                    lx + bx >= sw,
                    "{sw}x{sh} into {dw}x{dh}: right edge lost ({lx})"
                );
                assert!(
                    ly + by >= sh,
                    "{sw}x{sh} into {dw}x{dh}: BOTTOM edge lost ({ly}) — this is \
                     the dialogue-box crop"
                );
                // Sampling is unbiased: mirroring the output mirrors the
                // source, to within the one pixel integers cost. Sampling at
                // each output pixel's left edge instead of its centre passes
                // every check above and still leans the whole picture towards
                // the top-left, which on a shrunk frame loses the bottom row.
                let live = |m: &[u32]| -> Vec<u32> {
                    m.iter().copied().filter(|&v| v != u32::MAX).collect()
                };
                for (m, s) in [(live(&xm), sw), (live(&ym), sh)] {
                    let n = m.len();
                    for i in 0..n {
                        let sum = m[i] as i64 + m[n - 1 - i] as i64;
                        assert!(
                            (sum - (s as i64 - 1)).abs() <= 1,
                            "{sw}x{sh} into {dw}x{dh}: sampling is lopsided at \
                             {i} ({} + {} != {})",
                            m[i],
                            m[n - 1 - i],
                            s - 1
                        );
                    }
                }
            }
        }
    }

    /// Integer magnification wherever it divides, because a fractional one
    /// gives pixel art uneven pixel widths and the crisp look is the point.
    #[test]
    fn fit_uses_whole_pixels_whenever_the_frame_allows_it() {
        for (dw, dh, want) in [
            (640u32, 480u32, (480u32, 432u32)), // x3
            (1280, 720, (800, 720)),            // x5, exactly full height
            (1024, 576, (640, 576)),            // x4, exactly full height
            (768, 432, (480, 432)),             // x3, exactly full height
            (1920, 1080, (1120, 1008)),         // x7 — 1080/144 is 7.5, so x7
        ] {
            let (tw, th) = fit_size(160, 144, dw, dh);
            assert_eq!((tw, th), want, "160x144 into {dw}x{dh}");
            assert_eq!(tw % 160, 0, "width is a whole multiple");
            assert_eq!(th % 144, 0, "height is a whole multiple");
            // Nearest-neighbour with no resampling: every output column maps to
            // a source column, in runs of exactly `k`.
            let (xm, _) = fit_maps(160, 144, dw, dh);
            let k = tw / 160;
            let (ox, _) = fit_origin(tw, th, dw, dh);
            for x in 0..tw {
                assert_eq!(xm[(ox + x) as usize], x / k, "column {x} of {dw}x{dh}");
            }
        }
    }

    /// The card and the game are two pictures in the same frame, so they must
    /// occupy the same rectangle. They did not: the card was drawn edge to edge
    /// while the game was letterboxed, so pressing `v` made the feed jump.
    #[test]
    fn the_idle_card_lands_in_the_same_rectangle_as_the_game() {
        for (dw, dh) in [(640u32, 480u32), (1280, 720), (854, 480), (641, 481)] {
            for (sw, sh) in [(160u32, 144u32), (320, 288)] {
                let mut v = VideoOut::in_memory(spec(
                    PixelFormat::Rgba,
                    Pace::Emulated,
                    Some((dw, dh)),
                ));
                let t = Instant::now();
                // A game frame that is white everywhere, so "not a bar" is
                // exactly "the picture".
                v.frame(&solid(sw, sh, [255, 255, 255, 255]), sw, sh, t);
                let frame_px = (dw * dh * 4) as usize;
                let game = content_box(&v.captured()[..frame_px], dw, dh);

                v.go_idle();
                v.idle(t);
                let card = content_box(&v.captured()[frame_px..frame_px * 2], dw, dh);

                assert_eq!(
                    game, card,
                    "{sw}x{sh} in {dw}x{dh}: the card sits in a different box \
                     from the game, so switching jumps"
                );
                let (x0, y0, x1, y1) = card;
                assert_eq!(
                    (x1 - x0, y1 - y0),
                    fit_size(sw, sh, dw, dh),
                    "and that box is the fitted one"
                );
            }
        }
    }

    /// The crop the captain saw is the *consumer's*, and this warning is the
    /// only place it can be caught — the frame on the wire is already correct.
    #[test]
    fn a_camera_that_is_not_sixteen_by_nine_is_warned_about() {
        assert!(v4l2::aspect_warning(640, 480).is_some(), "4:3 is cropped");
        assert!(v4l2::aspect_warning(320, 288).is_some(), "10:9 is cropped");
        assert!(v4l2::aspect_warning(480, 480).is_some(), "1:1 is cropped");
        assert_eq!(v4l2::aspect_warning(1280, 720), None);
        assert_eq!(v4l2::aspect_warning(1920, 1080), None);
        assert_eq!(
            v4l2::aspect_warning(854, 480),
            None,
            "16:9 to within a pixel"
        );
        assert_eq!(v4l2::aspect_warning(0, 0), None, "no geometry, no claim");
        let w = v4l2::aspect_warning(640, 480).unwrap();
        assert!(w.contains("1280x720"), "names the size to use: {w}");
        // The first line has to stand on its own — it is the whole of the
        // toast, and a toast that stops mid-sentence teaches nobody anything.
        let head = w.lines().next().unwrap();
        assert!(
            head.len() <= 100,
            "toast line is {} chars: {head}",
            head.len()
        );
        assert!(
            head.contains("crop"),
            "the toast line names the symptom: {head}"
        );
        // The default must be the thing the warning recommends, or the advice
        // and the product disagree.
        assert_eq!(
            v4l2::aspect_warning(v4l2::DEFAULT_SIZE.0, v4l2::DEFAULT_SIZE.1),
            None
        );
    }

    #[test]
    fn geometry_is_fixed_by_the_first_frame_and_survives_a_size_change() {
        let mut v = VideoOut::in_memory(spec(
            PixelFormat::Rgba,
            Pace::Emulated,
            Some((640, 480)),
        ));
        let t = Instant::now();
        v.frame(&solid(160, 144, [1, 2, 3, 255]), 160, 144, t);
        // The user cycles the upscaler mid-stream: a 320x288 frame arrives.
        v.frame(&solid(320, 288, [4, 5, 6, 255]), 320, 288, t);
        assert_eq!(v.geometry(), Some((640, 480)));
        assert_eq!(
            v.captured().len(),
            2 * 640 * 480 * 4,
            "every frame is the same size or the consumer desynchronises"
        );
    }

    #[test]
    fn the_stream_is_exactly_frames_times_frame_size() {
        for (fmt, bpp) in [
            (PixelFormat::Rgba, 4),
            (PixelFormat::Rgb24, 3),
            (PixelFormat::Yuyv422, 2),
        ] {
            let mut v = VideoOut::in_memory(spec(fmt, Pace::Emulated, Some((320, 240))));
            let t = Instant::now();
            for _ in 0..3 {
                v.frame(&solid(160, 144, [9, 9, 9, 255]), 160, 144, t);
            }
            assert_eq!(
                v.captured().len(),
                3 * 320 * 240 * bpp,
                "{} stream must be frame-aligned",
                fmt.ffmpeg_name()
            );
        }
    }

    #[test]
    fn a_closed_sink_disables_the_stream_instead_of_failing_forever() {
        // /dev/full accepts an open and fails every write.
        let (mut v, err) = VideoOut::open(VideoSpec {
            dest: Destination::Path(PathBuf::from("/dev/full")),
            ..spec(PixelFormat::Rgba, Pace::Emulated, None)
        });
        if err.is_some() {
            return; // no /dev/full on this platform
        }
        let t = Instant::now();
        v.frame(&solid(160, 144, [1, 1, 1, 255]), 160, 144, t);
        assert!(!v.is_enabled(), "a failing sink must switch itself off");
        assert_eq!(v.stats().write_errors, 1);
        v.frame(&solid(160, 144, [1, 1, 1, 255]), 160, 144, t);
        assert_eq!(v.stats().write_errors, 1, "and must not retry every frame");
    }

    /// A V4L2 node that is not there — the common case on a machine with no
    /// v4l2loopback — must arm and wait, never block and never fail the run.
    /// And the message has to be the one remaining setup step, actionable.
    #[test]
    #[cfg(all(target_os = "linux", feature = "libc"))]
    fn a_missing_camera_does_not_stop_the_emulator_and_says_what_to_install() {
        // A number no machine has, so this is deterministic everywhere.
        let t = Instant::now();
        let (v, note) = VideoOut::open(VideoSpec {
            dest: Destination::Path(PathBuf::from("/dev/video233")),
            ..spec(PixelFormat::Yuyv422, Pace::Realtime, Some((640, 480)))
        });
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "opening a missing camera took {:?}",
            t.elapsed()
        );
        assert!(v.is_enabled(), "armed: the device may yet appear");
        assert_eq!(v.state(), StreamState::Waiting);
        assert!(v.is_camera());
        assert_eq!(v.intent(), Intent::Stream);
        let note = note.expect("must say why, not fail silently");
        if !v4l2::module_loaded() {
            assert!(note.contains("v4l2loopback"), "{note}");
            assert!(note.contains("modprobe"), "{note}");
        }
    }

    /// **Detection over configuration.** A camera has one shape that works;
    /// the user must not have to know it, and a default that cannot work must
    /// not be kept silently.
    #[test]
    #[cfg(all(target_os = "linux", feature = "libc"))]
    fn a_camera_gets_yuyv_and_a_size_consumers_accept_without_being_asked() {
        let (v, note) = VideoOut::open(VideoSpec {
            dest: Destination::Path(PathBuf::from("/dev/video233")),
            // Deliberately the defaults that cannot drive a camera.
            ..spec(PixelFormat::Rgba, Pace::Realtime, None)
        });
        assert_eq!(v.spec().format, PixelFormat::Yuyv422, "forced to YUYV");
        assert_eq!(v.spec().size, Some(v4l2::DEFAULT_SIZE));
        assert_eq!(v4l2::DEFAULT_SIZE, (1280, 720));
        // The default format is silently corrected — nobody typed it. A format
        // the user *did* choose is not silently corrected.
        let quiet = note.unwrap_or_default();
        assert!(!quiet.contains("does not apply"), "{quiet}");
        let (v, note) = VideoOut::open(VideoSpec {
            dest: Destination::Path(PathBuf::from("/dev/video233")),
            ..spec(PixelFormat::Rgb24, Pace::Realtime, None)
        });
        assert_eq!(v.spec().format, PixelFormat::Yuyv422);
        let note = note.unwrap_or_default();
        assert!(note.contains("does not apply"), "{note}");
        assert!(note.contains("rgb24"), "{note}");
    }

    /// The struct layouts are hand-written from the kernel UAPI, so the request
    /// numbers they generate are the proof they are right. (Also `const`-
    /// asserted at compile time — this fails the *test* run too, so a reader
    /// grepping for the numbers finds them.)
    #[test]
    #[cfg(all(target_os = "linux", feature = "libc"))]
    fn the_v4l2_ioctl_numbers_match_the_kernel_headers() {
        assert_eq!(v4l2::sink::VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(v4l2::sink::VIDIOC_S_FMT, 0xc0d0_5605);
        assert_eq!(v4l2::sink::PIX_FMT_YUYV, 0x5659_5559);
        assert_eq!(v4l2::sink::fourcc_name(v4l2::sink::PIX_FMT_YUYV), "YUYV");
    }

    #[test]
    fn the_hint_matches_what_is_actually_written() {
        let mut v = VideoOut::in_memory(spec(
            PixelFormat::Yuyv422,
            Pace::Emulated,
            Some((640, 480)),
        ));
        v.frame(&solid(160, 144, [1, 1, 1, 255]), 160, 144, Instant::now());
        let hint = v.ffmpeg_hint();
        assert!(hint.contains("-pix_fmt yuyv422"), "{hint}");
        assert!(hint.contains("-s 640x480"), "{hint}");
        assert!(hint.contains("-framerate 59.7275"), "{hint}");
        // The claim in the hint is the truth about the bytes.
        assert_eq!(v.captured().len(), 640 * 480 * 2);
    }

    // ---- FIFO destinations -------------------------------------------------
    //
    // The defect these pin: `video_out` pointing at a FIFO with no reader made
    // the emulator hang in `open(2)` before any UI existed, so the only thing
    // the user could do was kill it. A media sink must never be able to stop
    // the emulator from starting.

    /// A FIFO in a private directory, plus its cleanup. `None` on a platform
    /// (or a build) that cannot make one, so the whole group skips rather than
    /// failing for the wrong reason.
    #[cfg(all(unix, feature = "libc"))]
    fn temp_fifo(tag: &str) -> Option<PathBuf> {
        use std::ffi::CString;
        let dir =
            std::env::temp_dir().join(format!("gbvideo-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("gb.raw");
        let _ = std::fs::remove_file(&path);
        let c = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
            return None;
        }
        Some(path)
    }

    #[cfg(all(unix, feature = "libc"))]
    fn scrub(path: &Path) {
        if let Some(d) = path.parent() {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Open the reading end without blocking on the writer.
    #[cfg(all(unix, feature = "libc"))]
    fn reader(path: &Path) -> File {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .expect("a FIFO always opens for reading")
    }

    /// **The regression test.** Opening a FIFO that nobody is reading must
    /// return, promptly, with the stream armed and waiting — not park the
    /// process in `open(2)` forever.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_fifo_with_no_reader_does_not_block_startup() {
        let Some(path) = temp_fifo("noreader") else {
            return;
        };
        let t = Instant::now();
        let (v, note) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            ..spec(PixelFormat::Rgba, Pace::Emulated, None)
        });
        let took = t.elapsed();
        assert!(
            took < Duration::from_secs(1),
            "opening an unread FIFO took {took:?} — it must not wait for a reader"
        );
        assert_eq!(v.state(), StreamState::Waiting);
        assert!(v.is_enabled(), "the stream is armed, just not live yet");
        assert_eq!(v.intent(), Intent::Stream);
        let note = note.expect("the user must be told it is waiting, not stuck");
        assert!(note.contains("waiting for a consumer"), "{note}");
        scrub(&path);
    }

    /// Frames offered while nobody is reading must also return promptly, and
    /// must not be counted as delivered.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn frames_offered_to_an_unread_fifo_are_free_and_not_counted() {
        let Some(path) = temp_fifo("unread") else {
            return;
        };
        let (mut v, _) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            ..spec(PixelFormat::Rgba, Pace::Emulated, None)
        });
        let f = solid(160, 144, [1, 2, 3, 255]);
        let t = Instant::now();
        for _ in 0..120 {
            v.frame(&f, 160, 144, Instant::now());
        }
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "120 frames at an unread FIFO took {:?}",
            t.elapsed()
        );
        let s = v.stats();
        assert_eq!(s.frames, 0, "nothing was delivered");
        assert_eq!(s.bytes, 0);
        assert!(s.unconsumed > 0, "and it is visible that nothing was");
        assert_eq!(v.state(), StreamState::Waiting);
        scrub(&path);
    }

    /// The captain's order of operations: emulator first, bridge second. The
    /// stream must go live on its own when the consumer turns up.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_consumer_that_arrives_late_is_picked_up() {
        let Some(path) = temp_fifo("late") else {
            return;
        };
        let (mut v, _) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            ..spec(PixelFormat::Rgba, Pace::Emulated, Some((16, 16)))
        });
        let f = solid(16, 16, [9, 8, 7, 255]);
        v.frame(&f, 16, 16, Instant::now());
        assert_eq!(v.state(), StreamState::Waiting);

        // ffmpeg starts.
        let mut r = reader(&path);
        // The retry is rate-limited, so hand it an instant past the backoff
        // rather than sleeping.
        v.frame(&f, 16, 16, Instant::now() + RETRY_EVERY);
        assert_eq!(v.state(), StreamState::Live, "a late consumer must attach");
        assert_eq!(v.stats().frames, 1);
        assert_eq!(v.stats().attaches, 1);

        let mut buf = vec![0u8; 16 * 16 * 4];
        use std::io::Read as _;
        assert_eq!(
            r.read(&mut buf).unwrap(),
            16 * 16 * 4,
            "the consumer sees a whole frame"
        );
        assert_eq!(&buf[..4], &[9, 8, 7, 255]);
        scrub(&path);
    }

    /// And the consumer leaving must cost the stream nothing but its `Live`
    /// state — no crash, no permanent disable, and the next one is picked up.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_consumer_that_leaves_drops_back_to_waiting_and_can_be_replaced() {
        let Some(path) = temp_fifo("leave") else {
            return;
        };
        let (mut v, _) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            ..spec(PixelFormat::Rgba, Pace::Emulated, Some((16, 16)))
        });
        let f = solid(16, 16, [1, 1, 1, 255]);
        let mut r = reader(&path);
        v.frame(&f, 16, 16, Instant::now() + RETRY_EVERY);
        assert_eq!(v.state(), StreamState::Live);

        // The bridge is killed. Writing at a pipe with no reader raises
        // SIGPIPE unless it is ignored — if this test dies rather than fails,
        // `pipe::ignore_sigpipe` stopped working.
        drop(r);
        for _ in 0..3 {
            v.frame(&f, 16, 16, Instant::now());
        }
        assert!(v.is_enabled(), "the stream must survive its consumer");
        assert_eq!(v.state(), StreamState::Waiting);
        assert_eq!(v.stats().detaches, 1);

        // A new bridge starts.
        r = reader(&path);
        v.frame(&f, 16, 16, Instant::now() + RETRY_EVERY * 40);
        assert_eq!(v.state(), StreamState::Live, "and pick up the next one");
        assert_eq!(v.stats().attaches, 2);
        drop(r);
        scrub(&path);
    }

    /// A consumer that stops draining fills the pipe buffer. That must cost
    /// frames, not frame time: the emulator is not allowed to wait for ffmpeg.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_stalled_consumer_costs_frames_never_emulation_time() {
        let Some(path) = temp_fifo("stall") else {
            return;
        };
        let (mut v, _) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            ..spec(PixelFormat::Rgba, Pace::Emulated, Some((320, 288)))
        });
        // A reader that attaches and then never reads a byte — the pipe buffer
        // (64 KiB by default) fills after the first frame or two.
        let r = reader(&path);
        let f = solid(320, 288, [4, 4, 4, 255]);
        v.frame(&f, 320, 288, Instant::now() + RETRY_EVERY);
        assert_eq!(v.state(), StreamState::Live);

        let t = Instant::now();
        for _ in 0..200 {
            v.frame(&f, 320, 288, Instant::now());
        }
        let took = t.elapsed();
        assert!(
            took < Duration::from_millis(500),
            "200 frames at a stalled consumer took {took:?} — emulation was blocked"
        );
        let s = v.stats();
        assert!(s.stalled > 0, "frames must be dropped: {s:?}");
        assert!(
            s.frames < 201,
            "a stalled consumer cannot have received everything"
        );
        // Still live, still armed: a consumer that resumes keeps its stream.
        assert_eq!(v.state(), StreamState::Live);
        drop(r);
        scrub(&path);
    }

    /// The consumer must never be handed a frame boundary that is not one:
    /// `rawvideo` is positional, so half of frame N followed by frame N+1
    /// desynchronises everything after it.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_partial_write_is_finished_before_the_next_frame_starts() {
        let Some(path) = temp_fifo("align") else {
            return;
        };
        let (mut v, _) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            ..spec(PixelFormat::Rgba, Pace::Emulated, Some((160, 144)))
        });
        let bytes = 160 * 144 * 4; // 92 KiB — larger than a 64 KiB pipe buffer
        let mut r = reader(&path);
        // Distinct frames, so a desynchronised stream is visible in the data.
        let a = solid(160, 144, [0xA1, 0xA2, 0xA3, 255]);
        let b = solid(160, 144, [0xB1, 0xB2, 0xB3, 255]);
        v.frame(&a, 160, 144, Instant::now() + RETRY_EVERY);
        // The pipe took part of `a`; offering `b` now must not interleave.
        v.frame(&b, 160, 144, Instant::now());

        use std::io::Read as _;
        let mut got = Vec::new();
        let mut buf = vec![0u8; 65536];
        for _ in 0..40 {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
            // Draining lets the writer make progress on the outstanding tail.
            v.idle(Instant::now());
            if got.len() >= bytes {
                break;
            }
        }
        assert!(got.len() >= bytes, "read {} of {bytes}", got.len());
        assert!(
            got[..bytes]
                .chunks_exact(4)
                .all(|p| p == [0xA1, 0xA2, 0xA3, 255]),
            "the first frame must arrive whole and unmixed"
        );
        drop(r);
        scrub(&path);
    }

    /// The sidecar is what stops the consumer being configured from a second,
    /// drifting copy of the same numbers.
    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn a_fifo_publishes_a_descriptor_the_bridge_can_read() {
        let Some(path) = temp_fifo("descriptor") else {
            return;
        };
        let (_v, _) = VideoOut::open(VideoSpec {
            dest: Destination::Path(path.clone()),
            title: Some("POKEMON BLUE\0\0\0".to_string()),
            ..spec(PixelFormat::Yuyv422, Pace::Realtime, Some((640, 480)))
        });
        let text = std::fs::read_to_string(descriptor_path(&path))
            .expect("a FIFO stream must publish its shape");
        for line in [
            "width=640",
            "height=480",
            "format=yuyv422",
            "title=POKEMON BLUE",
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line:?}:\n{text}");
        }
        assert!(text.contains(&format!("fps={DMG_HZ}")), "{text}");
        scrub(&path);
    }

    #[test]
    fn a_recording_and_a_live_stream_are_different_intents() {
        // Derived from what the destination is, never guessed.
        assert_eq!(Destination::Stdout.intent(), Intent::Stream);
        assert_eq!(
            Destination::Path(PathBuf::from("/tmp/does-not-exist.raw")).intent(),
            Intent::Record
        );
        assert_eq!(Intent::Stream.word(), "LIVE");
        assert_eq!(Intent::Record.word(), "REC");
        assert_ne!(Intent::Stream.word(), Intent::Record.word());
    }

    #[test]
    fn odd_widths_are_rounded_up_for_yuyv_with_a_complaint() {
        let s = VideoSpec::new(Destination::Stdout);
        let mut warn = Vec::new();
        let s = VideoSpec {
            format: PixelFormat::Yuyv422,
            ..s
        };
        assert_eq!(s.even_width(641, 480, &mut warn), (642, 480));
        assert_eq!(warn.len(), 1);
        assert!(warn[0].contains("odd width"), "{:?}", warn);
    }
}

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
//! | frame size | `GB_VIDEO_SIZE` | `video_size` | `WxH`, e.g. `640x480` |
//! | picture source | `GB_VIDEO_SOURCE` | `video_source` | `native` (default), `display` |
//! | pacing | `GB_VIDEO_PACE` | `video_pace` | `realtime` (default), `emulated` |
//!
//! The environment wins over the config file, as with telemetry.
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
use std::path::PathBuf;
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
    pub fn describe(&self, geometry: Option<(u32, u32)>) -> String {
        let (w, h) = geometry.or(self.size).unwrap_or((SRC_W, SRC_H));
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
    "  --video-out <-|PATH>      stream raw frames (`-` = stdout, or a file/FIFO)\n  \
     --video-format <fmt>      rgba (default), rgb24, yuyv\n  \
     --video-size <WxH>        letterbox into this size, e.g. 640x480\n  \
     --video-source <src>      native (default) or display (palette + upscaler)\n  \
     --video-pace <pace>       realtime (default) or emulated"
}

// ---------------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------------

enum Sink {
    Stdout(std::io::Stdout),
    File(File),
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
            stats: VideoStats::default(),
        }
    }

    /// Open `spec`. Never takes a run down with it: a destination that cannot
    /// be opened yields a disabled emitter and an explanation.
    ///
    /// Two destinations get a *specific* explanation rather than a bare OS
    /// error, because both are things a user will reasonably try:
    ///
    /// * a `/dev/video*` node — writing raw bytes at a V4L2 device does not
    ///   work until its format has been negotiated; the supported route is
    ///   `ffmpeg -f v4l2` (see [`v4l2`]).
    /// * a FIFO — opening one for writing blocks until a reader appears, which
    ///   otherwise looks exactly like the emulator hanging on startup.
    pub fn open(spec: VideoSpec) -> (VideoOut, Option<String>) {
        if let Destination::Path(p) = &spec.dest {
            if v4l2::is_video_device(p) {
                return (
                    VideoOut::disabled(),
                    Some(v4l2::direct_write_refusal(p, &spec)),
                );
            }
        }
        let sink = match &spec.dest {
            Destination::Stdout => Ok(Sink::Stdout(std::io::stdout())),
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
                v.spec = spec;
                v.sink = Some(s);
                (v, None)
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

    /// `true` when frames are actually going somewhere. Callers check this
    /// before doing any work that is not free — including reading the clock.
    pub fn is_enabled(&self) -> bool {
        self.sink.is_some()
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
        if self.sink.is_none() {
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
        if self.sink.is_none() || self.staged.is_empty() {
            return;
        }
        self.pump(now, false);
    }

    fn stage(&mut self, rgba: &[u8], w: u32, h: u32) {
        if self.staged.len() != rgba.len() {
            self.staged.resize(rgba.len(), 0);
        }
        self.staged.copy_from_slice(rgba);
        self.staged_dims = (w, h);
        self.wire_valid = false;
    }

    fn pump(&mut self, now: Instant, fresh: bool) {
        match self.spec.pace {
            Pace::Emulated => {
                if fresh {
                    self.emit();
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
                    self.emit();
                }
            }
        }
    }

    /// Convert the staged frame (once) and write it.
    fn emit(&mut self) {
        let t = Instant::now();
        if !self.wire_valid {
            self.convert();
            self.wire_valid = true;
        }
        let ok = match self.sink.as_mut() {
            Some(Sink::Stdout(o)) => {
                let mut lock = o.lock();
                lock.write_all(&self.wire)
                    .and_then(|_| lock.flush())
                    .is_ok()
            }
            Some(Sink::File(f)) => f.write_all(&self.wire).is_ok(),
            Some(Sink::Memory(v)) => {
                v.extend_from_slice(&self.wire);
                true
            }
            None => true,
        };
        if ok {
            self.stats.frames += 1;
            self.stats.bytes += self.wire.len() as u64;
        } else {
            // A closed pipe (the consumer quit) must not take the game down and
            // must not cost a syscall per frame for the rest of the run.
            self.stats.write_errors += 1;
            self.sink = None;
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
            if self.xmap.len() != ow as usize || self.ymap.len() != oh as usize {
                let (xm, ym) = fit_maps(sw, sh, ow, oh);
                self.xmap = xm;
                self.ymap = ym;
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

    /// The exact command that consumes this stream. Printed at startup so the
    /// answer to "how do I read this?" ships with the stream itself.
    pub fn ffmpeg_hint(&self) -> String {
        self.spec.ffmpeg_hint(self.out)
    }

    /// One line describing the live stream, for the frontend to show.
    pub fn describe(&self) -> String {
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
            _ => {}
        }
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
/// stretched, because 160×144 is 10:9 and every conventional camera size is
/// 4:3 or 16:9.
pub fn fit_maps(sw: u32, sh: u32, dw: u32, dh: u32) -> (Vec<u32>, Vec<u32>) {
    let (tw, th) = fit_size(sw, sh, dw, dh);
    let ox = (dw - tw) / 2;
    let oy = (dh - th) / 2;
    // `u32::MAX` marks a bar pixel (no source).
    let mut xmap = vec![u32::MAX; dw as usize];
    let mut ymap = vec![u32::MAX; dh as usize];
    for x in 0..tw {
        xmap[(ox + x) as usize] = x * sw / tw;
    }
    for y in 0..th {
        ymap[(oy + y) as usize] = y * sh / th;
    }
    (xmap, ymap)
}

/// The size the picture is drawn at inside `dw×dh`.
pub fn fit_size(sw: u32, sh: u32, dw: u32, dh: u32) -> (u32, u32) {
    let k = (dw / sw).min(dh / sh);
    if k >= 1 {
        return (sw * k, sh * k);
    }
    // Smaller than the source: fall back to an aspect-preserving fractional
    // fit (uneven pixels, but a legible picture beats a cropped one).
    if dw * sh <= dh * sw {
        (dw, (dw * sh / sw).max(1))
    } else {
        ((dh * sw / sh).max(1), dh)
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

/// Virtual-camera support: **detect and explain**, deliberately not implement.
///
/// Feeding a `v4l2loopback` node from inside the emulator means negotiating a
/// format over `VIDIOC_S_FMT` before the first `write()`. That is not hard, but
/// it is *untestable without the kernel module installed*, and the module needs
/// root to install — so shipping hand-written `ioctl` bindings that no test has
/// ever exercised would be worse than shipping nothing.
///
/// The supported route is one extra process, and it is strictly more capable
/// (it gets scaling, format conversion and MJPEG for free):
///
/// ```text
/// ffmpeg -f rawvideo -pix_fmt yuyv422 -s 640x480 -framerate 59.7275 -i /tmp/gb.raw \
///        -f v4l2 -pix_fmt yuyv422 /dev/video10
/// ```
///
/// `tools/virtual-camera.sh` does exactly this, including the detection below.
pub mod v4l2 {
    use super::VideoSpec;
    use std::path::Path;

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

    /// Why we will not write straight at `/dev/videoN`, and what to do instead.
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

    #[test]
    fn a_v4l2_node_is_refused_with_the_ffmpeg_recipe() {
        let (v, err) = VideoOut::open(VideoSpec {
            dest: Destination::Path(PathBuf::from("/dev/video10")),
            ..spec(PixelFormat::Yuyv422, Pace::Realtime, Some((640, 480)))
        });
        assert!(!v.is_enabled());
        let err = err.expect("must explain rather than fail obscurely");
        assert!(err.contains("-f v4l2"), "{err}");
        assert!(err.contains("mkfifo"), "{err}");
        assert!(err.contains("640x480"), "{err}");
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

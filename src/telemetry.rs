//! Machine-readable run telemetry — newline-delimited JSON.
//!
//! # Why
//!
//! An agent (or a CI job) working on this emulator cannot see the screen or
//! hear the speaker. Everything the on-screen stats overlay shows a human — fps,
//! frame time, emulated speed, the per-stage render breakdown — is therefore
//! also published here in a form a program can consume, so "did that change
//! actually help?" is answerable without a human looking.
//!
//! This is the first concrete implementation of the **emit** half of the public
//! extension surface designed in [`docs/debugging.md`](../docs/debugging.md):
//! the `Frame rendered` event, the one the design calls "always cheap — the
//! general capture primitive". It follows that design's rules:
//!
//! * **Opt-in.** Nothing is emitted unless a destination is configured, and the
//!   check is a single `Option` test per frame ([`Telemetry::is_enabled`]).
//! * **Serialisable messages, not sockets.** The record is JSON; the transport
//!   ([`Sink`]) is separable — today a file or stdout, later a socket or the
//!   wasm `postMessage` bridge, with the same record shape.
//! * **Never perturbs what it measures.** Timing uses values the frontend
//!   already computed; formatting one record is a few hundred bytes into a
//!   reused `String` and a buffered write. Disabled, it costs one branch.
//!
//! # Schema
//!
//! The record shape is a **stable contract**, documented in
//! [`docs/telemetry.md`](../docs/telemetry.md). Consumers must ignore unknown
//! fields; new fields may be added, existing ones are not renamed or retyped
//! without bumping [`SCHEMA_VERSION`].
//!
//! # Enabling
//!
//! * `GB_TELEMETRY=<path>` — append NDJSON to that file.
//! * `GB_TELEMETRY=-` or `stdout` — write to stdout (headless use only; in the
//!   TUI this would corrupt the alternate screen, so the terminal frontend
//!   refuses it and says so).
//! * `GB_TELEMETRY_EVERY=<n>` — emit one record every `n` frames (default 1).
//!
//! The terminal frontend also accepts a `telemetry = <path>` config key; the
//! environment variable wins.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::time::Duration;

/// Bumped when a field is renamed, retyped or removed (never for additions).
pub const SCHEMA_VERSION: u32 = 1;

/// Where records go.
pub enum Sink {
    Stdout,
    File(BufWriter<File>),
    /// In-memory, for tests and for the one-shot summary path.
    Memory(Vec<String>),
}

/// A destination parsed from configuration, before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Stdout,
    Path(PathBuf),
}

impl Destination {
    /// Parse a `GB_TELEMETRY` / `telemetry = …` value. Empty, `off`, `0` and
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

    /// Read the environment override, if any.
    pub fn from_env() -> Option<Destination> {
        std::env::var("GB_TELEMETRY")
            .ok()
            .as_deref()
            .and_then(Destination::parse)
    }
}

/// Static facts about the run, emitted with every record so a single line is
/// self-describing (an NDJSON consumer may only ever see one line).
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub rom_title: String,
    pub mbc: String,
    /// Frontend name: `"terminal"`, `"bench"`, …
    pub frontend: String,
    pub protocol: String,
    pub palette: String,
    pub upscale: String,
    pub upscale_factor: u8,
    /// Effective on-screen magnification of the 160×144 picture.
    pub scale: f32,
    /// Speed rung label (`"1x"`, `"MAX"`, …).
    pub speed: String,
    /// Terminal size in cells, and the display rectangle in cells.
    pub term_cols: u16,
    pub term_rows: u16,
    pub display_cols: u16,
    pub display_rows: u16,
    /// Pixel size of the image actually transmitted to the terminal, and of the
    /// area it is stretched over. The first is what costs time; the gap between
    /// them is the whole optimisation.
    pub tx_w: u32,
    pub tx_h: u32,
    pub display_px_w: u32,
    pub display_px_h: u32,
}

/// One frame's measurements. All durations are milliseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSample {
    /// Monotonic frame counter since the session started.
    pub frame: u64,
    /// Displayed frames per second, averaged over the last second.
    pub fps: f32,
    /// Emulated frames per second — the number that must stay at 59.7275.
    pub emulated_fps: f32,
    /// `emulated_fps / 59.7275 * 100`.
    pub speed_pct: f32,
    /// Wall-clock milliseconds for the whole loop iteration.
    pub frame_ms: f32,
    // Per-stage breakdown.
    pub emulate_ms: f32,
    pub palette_ms: f32,
    pub upscale_ms: f32,
    pub encode_ms: f32,
    pub write_ms: f32,
    /// Emulated frames advanced since the last displayed frame (>1 means the
    /// display is being skipped to keep emulation at real time).
    pub frames_skipped: u64,
    /// Cumulative displayed frames that were skipped because the framebuffer
    /// was unchanged (nothing needed retransmitting).
    pub frames_identical: u64,
    /// Cumulative emulated frames never shown because rendering was behind.
    pub frames_dropped: u64,
    /// Escape-sequence bytes handed to the terminal this frame.
    pub tx_bytes: usize,
    // Audio, when a host stream is attached (all zero otherwise).
    pub audio: Option<AudioSample>,
    /// Video-out, when a raw frame stream is attached (`None` otherwise).
    pub video: Option<VideoSample>,
}

/// Raw video-out health, read from [`crate::video::VideoOut::stats`]. Present
/// only when a stream is configured, which it is not by default.
#[derive(Debug, Clone, Copy, Default)]
pub struct VideoSample {
    /// Frames written to the stream so far (duplicates included).
    pub frames: u64,
    /// Frames repeated to hold the wall-clock rate (pause, slow motion).
    pub duplicated: u64,
    /// Emulated frames not represented in the stream (fast-forward).
    pub dropped: u64,
    pub bytes: u64,
    /// Milliseconds spent converting and writing the last frame — the number
    /// that proves the stream is not perturbing the frame rate.
    pub ms: f32,
}

/// Host audio queue health, read from the player's shared queue handle.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioSample {
    pub queue_depth: usize,
    pub underruns: u64,
    pub dropped: u64,
    pub callbacks: u64,
    pub sample_rate: u32,
}

/// The emitter. Cheap to hold; does nothing until a [`Sink`] is attached.
pub struct Telemetry {
    sink: Option<Sink>,
    every: u64,
    seq: u64,
    buf: String,
    /// Accumulators for the one-shot summary.
    summary: Summary,
    /// Records dropped because the sink errored (disk full, closed pipe).
    write_errors: u64,
}

#[derive(Default, Clone, Copy)]
struct Summary {
    n: u64,
    fps: f64,
    emulated_fps: f64,
    frame_ms: f64,
    emulate_ms: f64,
    palette_ms: f64,
    upscale_ms: f64,
    encode_ms: f64,
    write_ms: f64,
    tx_bytes: f64,
    worst_frame_ms: f32,
    identical: u64,
    dropped: u64,
    underruns: u64,
}

impl Default for Telemetry {
    fn default() -> Self {
        Telemetry::disabled()
    }
}

impl Telemetry {
    /// A no-op emitter.
    pub fn disabled() -> Telemetry {
        Telemetry {
            sink: None,
            every: 1,
            seq: 0,
            buf: String::new(),
            summary: Summary::default(),
            write_errors: 0,
        }
    }

    /// Open `dest`. Returns a disabled emitter (and the error text) if the file
    /// cannot be opened — telemetry must never take a run down with it.
    pub fn open(dest: &Destination) -> (Telemetry, Option<String>) {
        let sink = match dest {
            Destination::Stdout => Ok(Sink::Stdout),
            Destination::Path(p) => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map(|f| Sink::File(BufWriter::new(f)))
                .map_err(|e| format!("telemetry: cannot open {}: {e}", p.display())),
        };
        match sink {
            Ok(s) => {
                let mut t = Telemetry::disabled();
                t.sink = Some(s);
                t.every = std::env::var("GB_TELEMETRY_EVERY")
                    .ok()
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .filter(|n| *n > 0)
                    .unwrap_or(1);
                (t, None)
            }
            Err(e) => (Telemetry::disabled(), Some(e)),
        }
    }

    /// An emitter that keeps every record in memory (tests, one-shot mode).
    pub fn in_memory() -> Telemetry {
        let mut t = Telemetry::disabled();
        t.sink = Some(Sink::Memory(Vec::new()));
        t
    }

    /// Build from the environment, if `GB_TELEMETRY` is set.
    pub fn from_env() -> (Telemetry, Option<String>) {
        match Destination::from_env() {
            Some(d) => Telemetry::open(&d),
            None => (Telemetry::disabled(), None),
        }
    }

    /// `true` when records are actually going somewhere. The frontend checks
    /// this before doing any measurement work that is not free.
    pub fn is_enabled(&self) -> bool {
        self.sink.is_some()
    }

    /// Records captured by an in-memory sink.
    pub fn records(&self) -> &[String] {
        match &self.sink {
            Some(Sink::Memory(v)) => v,
            _ => &[],
        }
    }

    /// Emit one frame record. Always accumulates the summary (so
    /// [`Telemetry::summary_json`] is right even when sampling), writes only
    /// every `GB_TELEMETRY_EVERY`-th frame.
    pub fn frame(&mut self, ctx: &Context, s: &FrameSample) {
        if self.sink.is_none() {
            return;
        }
        self.accumulate(s);
        self.seq += 1;
        if self.every > 1 && self.seq % self.every != 0 {
            return;
        }
        self.buf.clear();
        write_frame_json(&mut self.buf, ctx, s);
        self.buf.push('\n');
        self.flush_buf();
    }

    fn flush_buf(&mut self) {
        let line = std::mem::take(&mut self.buf);
        let ok = match self.sink.as_mut() {
            Some(Sink::Stdout) => {
                let mut out = std::io::stdout().lock();
                out.write_all(line.as_bytes())
                    .and_then(|_| out.flush())
                    .is_ok()
            }
            Some(Sink::File(f)) => f.write_all(line.as_bytes()).is_ok(),
            Some(Sink::Memory(v)) => {
                v.push(line.trim_end().to_string());
                true
            }
            None => true,
        };
        if !ok {
            self.write_errors += 1;
        }
        self.buf = line;
    }

    fn accumulate(&mut self, s: &FrameSample) {
        let a = &mut self.summary;
        a.n += 1;
        a.fps += s.fps as f64;
        a.emulated_fps += s.emulated_fps as f64;
        a.frame_ms += s.frame_ms as f64;
        a.emulate_ms += s.emulate_ms as f64;
        a.palette_ms += s.palette_ms as f64;
        a.upscale_ms += s.upscale_ms as f64;
        a.encode_ms += s.encode_ms as f64;
        a.write_ms += s.write_ms as f64;
        a.tx_bytes += s.tx_bytes as f64;
        a.worst_frame_ms = a.worst_frame_ms.max(s.frame_ms);
        a.identical = s.frames_identical;
        a.dropped = s.frames_dropped;
        if let Some(au) = s.audio {
            a.underruns = au.underruns;
        }
    }

    /// One JSON object summarising the whole run — the "run N frames headless,
    /// print one line, exit" contract used by tests and CI.
    pub fn summary_json(&self, ctx: &Context) -> String {
        let a = &self.summary;
        let n = a.n.max(1) as f64;
        let mut s = String::with_capacity(768);
        s.push('{');
        let _ = write!(s, "\"schema\":{SCHEMA_VERSION},\"type\":\"summary\"");
        write_context(&mut s, ctx);
        let _ = write!(s, ",\"frames\":{}", a.n);
        push_f(&mut s, "fps_avg", a.fps / n);
        push_f(&mut s, "emulated_fps_avg", a.emulated_fps / n);
        push_f(
            &mut s,
            "speed_pct_avg",
            a.emulated_fps / n / 59.7275 * 100.0,
        );
        push_f(&mut s, "frame_ms_avg", a.frame_ms / n);
        push_f(&mut s, "frame_ms_worst", a.worst_frame_ms as f64);
        push_f(&mut s, "emulate_ms_avg", a.emulate_ms / n);
        push_f(&mut s, "palette_ms_avg", a.palette_ms / n);
        push_f(&mut s, "upscale_ms_avg", a.upscale_ms / n);
        push_f(&mut s, "encode_ms_avg", a.encode_ms / n);
        push_f(&mut s, "write_ms_avg", a.write_ms / n);
        push_f(&mut s, "tx_bytes_avg", a.tx_bytes / n);
        push_f(
            &mut s,
            "tx_mb_per_s",
            a.tx_bytes / n * 59.7275 / 1_048_576.0,
        );
        let _ = write!(
            s,
            ",\"frames_identical\":{},\"frames_dropped\":{},\"audio_underruns\":{},\"write_errors\":{}",
            a.identical, a.dropped, a.underruns, self.write_errors
        );
        s.push('}');
        s
    }

    /// Flush a buffered file sink (called on shutdown).
    pub fn flush(&mut self) {
        if let Some(Sink::File(f)) = self.sink.as_mut() {
            let _ = f.flush();
        }
    }
}

/// Serialise one frame record. Split out so the schema has exactly one
/// definition and the tests can call it directly.
pub fn write_frame_json(out: &mut String, ctx: &Context, s: &FrameSample) {
    out.push('{');
    let _ = write!(out, "\"schema\":{SCHEMA_VERSION},\"type\":\"frame\"");
    write_context(out, ctx);
    let _ = write!(out, ",\"frame\":{}", s.frame);
    push_f(out, "fps", s.fps as f64);
    push_f(out, "emulated_fps", s.emulated_fps as f64);
    push_f(out, "speed_pct", s.speed_pct as f64);
    push_f(out, "frame_ms", s.frame_ms as f64);
    push_f(out, "emulate_ms", s.emulate_ms as f64);
    push_f(out, "palette_ms", s.palette_ms as f64);
    push_f(out, "upscale_ms", s.upscale_ms as f64);
    push_f(out, "encode_ms", s.encode_ms as f64);
    push_f(out, "write_ms", s.write_ms as f64);
    let _ = write!(
        out,
        ",\"frames_skipped\":{},\"frames_identical\":{},\"frames_dropped\":{},\"tx_bytes\":{}",
        s.frames_skipped, s.frames_identical, s.frames_dropped, s.tx_bytes
    );
    match s.audio {
        Some(a) => {
            let _ = write!(
                out,
                ",\"audio\":{{\"queue_depth\":{},\"underruns\":{},\"dropped\":{},\"callbacks\":{},\"sample_rate\":{}}}",
                a.queue_depth, a.underruns, a.dropped, a.callbacks, a.sample_rate
            );
        }
        None => out.push_str(",\"audio\":null"),
    }
    // Host-audio liveness, emitted ALWAYS — including (especially) when
    // `audio` above is null, because "no queue" and "the device refused to
    // open" are the same silence to a listener and completely different bugs.
    // `state` is off|live|failed; `errors` counts cpal faults on a running
    // stream. Addition only, so no SCHEMA_VERSION bump. See docs/telemetry.md.
    {
        let a = crate::sound::host_status::snapshot();
        let _ = write!(
            out,
            ",\"audio_host\":{{\"state\":\"{}\",\"errors\":{},\"error\":",
            a.state_name(),
            a.stream_errors
        );
        match &a.last_error {
            Some(e) => {
                let _ = write!(out, "\"{}\"", escape(e));
            }
            None => out.push_str("null"),
        }
        out.push('}');
    }
    match s.video {
        Some(v) => {
            let _ = write!(
                out,
                ",\"video\":{{\"frames\":{},\"duplicated\":{},\"dropped\":{},\"bytes\":{},\"ms\":{:.3}}}",
                v.frames, v.duplicated, v.dropped, v.bytes,
                if v.ms.is_finite() { v.ms } else { 0.0 }
            );
        }
        None => out.push_str(",\"video\":null"),
    }
    out.push('}');
}

fn write_context(out: &mut String, c: &Context) {
    let _ = write!(out, ",\"frontend\":\"{}\"", escape(&c.frontend));
    let _ = write!(out, ",\"rom\":\"{}\"", escape(&c.rom_title));
    let _ = write!(out, ",\"mbc\":\"{}\"", escape(&c.mbc));
    let _ = write!(out, ",\"protocol\":\"{}\"", escape(&c.protocol));
    let _ = write!(out, ",\"palette\":\"{}\"", escape(&c.palette));
    let _ = write!(
        out,
        ",\"upscale\":\"{}\",\"upscale_factor\":{}",
        escape(&c.upscale),
        c.upscale_factor
    );
    let _ = write!(out, ",\"speed\":\"{}\"", escape(&c.speed));
    push_f(out, "scale", c.scale as f64);
    let _ = write!(
        out,
        ",\"term_cols\":{},\"term_rows\":{},\"display_cols\":{},\"display_rows\":{}",
        c.term_cols, c.term_rows, c.display_cols, c.display_rows
    );
    let _ = write!(
        out,
        ",\"tx_w\":{},\"tx_h\":{},\"display_px_w\":{},\"display_px_h\":{}",
        c.tx_w, c.tx_h, c.display_px_w, c.display_px_h
    );
}

/// Numbers are written with three decimals — enough for microsecond-scale
/// stage timings — and never as `NaN`/`Infinity`, which are not valid JSON.
fn push_f(out: &mut String, key: &str, v: f64) {
    let v = if v.is_finite() { v } else { 0.0 };
    let _ = write!(out, ",\"{key}\":{v:.3}");
}

/// Minimal JSON string escaping (ROM titles are cartridge bytes and can be
/// anything).
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Milliseconds as `f32`, the unit every duration in the schema uses.
pub fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            rom_title: "ZELDA".into(),
            mbc: "MBC5".into(),
            frontend: "terminal".into(),
            protocol: "kitty".into(),
            palette: "green".into(),
            upscale: "nearest".into(),
            upscale_factor: 1,
            scale: 14.4,
            speed: "1x".into(),
            term_cols: 200,
            term_rows: 60,
            display_cols: 160,
            display_rows: 55,
            tx_w: 160,
            tx_h: 144,
            display_px_w: 2304,
            display_px_h: 2075,
        }
    }

    #[test]
    fn destination_parses_and_disables() {
        assert_eq!(Destination::parse("-"), Some(Destination::Stdout));
        assert_eq!(Destination::parse("StdOut"), Some(Destination::Stdout));
        assert_eq!(
            Destination::parse("/tmp/t.ndjson"),
            Some(Destination::Path(PathBuf::from("/tmp/t.ndjson")))
        );
        for off in ["", "  ", "off", "OFF", "false", "0"] {
            assert_eq!(Destination::parse(off), None, "{off:?}");
        }
    }

    #[test]
    fn disabled_emitter_does_nothing() {
        let mut t = Telemetry::disabled();
        assert!(!t.is_enabled());
        t.frame(&ctx(), &FrameSample::default());
        assert!(t.records().is_empty());
    }

    #[test]
    fn frame_record_has_every_contracted_field() {
        let mut t = Telemetry::in_memory();
        let s = FrameSample {
            frame: 42,
            fps: 59.9,
            emulated_fps: 59.72,
            speed_pct: 100.0,
            frame_ms: 16.7,
            emulate_ms: 1.2,
            palette_ms: 0.1,
            upscale_ms: 0.0,
            encode_ms: 0.4,
            write_ms: 0.2,
            frames_skipped: 0,
            frames_identical: 7,
            frames_dropped: 3,
            tx_bytes: 92_160,
            audio: Some(AudioSample {
                queue_depth: 6100,
                underruns: 0,
                dropped: 0,
                callbacks: 900,
                sample_rate: 48_000,
            }),
            video: None,
        };
        t.frame(&ctx(), &s);
        let line = &t.records()[0];
        for field in [
            "\"schema\":1",
            "\"type\":\"frame\"",
            "\"frontend\":\"terminal\"",
            "\"rom\":\"ZELDA\"",
            "\"mbc\":\"MBC5\"",
            "\"protocol\":\"kitty\"",
            "\"palette\":\"green\"",
            "\"upscale\":\"nearest\"",
            "\"upscale_factor\":1",
            "\"speed\":\"1x\"",
            "\"scale\":14.400",
            "\"tx_w\":160",
            "\"display_px_w\":2304",
            "\"fps\":59.900",
            "\"emulated_fps\":59.720",
            "\"speed_pct\":100.000",
            "\"emulate_ms\":1.200",
            "\"upscale_ms\":0.000",
            "\"encode_ms\":0.400",
            "\"write_ms\":0.200",
            "\"frames_identical\":7",
            "\"frames_dropped\":3",
            "\"tx_bytes\":92160",
            "\"queue_depth\":6100",
            "\"sample_rate\":48000",
        ] {
            assert!(line.contains(field), "missing {field} in {line}");
        }
        assert!(line.ends_with('}') && line.starts_with('{'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn audio_absent_is_json_null() {
        let mut t = Telemetry::in_memory();
        t.frame(&ctx(), &FrameSample::default());
        assert!(t.records()[0].contains("\"audio\":null"));
    }

    /// A silent run must still say *why* it is silent. `audio` being `null`
    /// only means "no queue handle"; `audio_host` is what distinguishes a build
    /// with no player from a device that refused to open, and it is emitted
    /// unconditionally so a capture can be checked without a human listening.
    #[test]
    fn host_audio_state_is_always_reported_even_with_no_stream() {
        use crate::sound::host_status;
        let _g = host_status::test_lock();
        host_status::reset();
        let mut t = Telemetry::in_memory();
        t.frame(&ctx(), &FrameSample::default());
        assert!(t.records()[0].contains("\"audio\":null"));
        assert!(t.records()[0]
            .contains("\"audio_host\":{\"state\":\"off\",\"errors\":0,\"error\":null}"));

        host_status::set_failed("no default output device");
        let mut t = Telemetry::in_memory();
        t.frame(&ctx(), &FrameSample::default());
        assert!(t.records()[0]
            .contains("\"audio_host\":{\"state\":\"failed\",\"errors\":0,\"error\":\"no default output device\"}"));
        host_status::reset();
    }

    #[test]
    fn video_out_is_reported_when_streaming_and_null_when_not() {
        let mut t = Telemetry::in_memory();
        t.frame(&ctx(), &FrameSample::default());
        assert!(t.records()[0].contains("\"video\":null"));

        let mut t = Telemetry::in_memory();
        t.frame(
            &ctx(),
            &FrameSample {
                video: Some(VideoSample {
                    frames: 120,
                    duplicated: 2,
                    dropped: 5,
                    bytes: 11_059_200,
                    ms: 0.12,
                }),
                ..Default::default()
            },
        );
        let line = &t.records()[0];
        for field in [
            "\"video\":{",
            "\"frames\":120",
            "\"duplicated\":2",
            "\"dropped\":5",
            "\"bytes\":11059200",
            "\"ms\":0.120",
        ] {
            assert!(line.contains(field), "missing {field} in {line}");
        }
    }

    #[test]
    fn non_finite_values_stay_valid_json() {
        let mut out = String::new();
        let s = FrameSample {
            fps: f32::NAN,
            frame_ms: f32::INFINITY,
            ..Default::default()
        };
        write_frame_json(&mut out, &ctx(), &s);
        assert!(!out.contains("NaN") && !out.contains("inf"));
        assert!(out.contains("\"fps\":0.000"));
    }

    #[test]
    fn titles_with_quotes_are_escaped() {
        let mut c = ctx();
        c.rom_title = "SUPER \"MARIO\"\n".into();
        let mut out = String::new();
        write_frame_json(&mut out, &c, &FrameSample::default());
        assert!(out.contains(r#"\"MARIO\"\n"#), "{out}");
    }

    #[test]
    fn sampling_writes_every_nth_but_summarises_all() {
        let mut t = Telemetry::in_memory();
        t.every = 3;
        for i in 0..9 {
            t.frame(
                &ctx(),
                &FrameSample {
                    frame: i,
                    frame_ms: 10.0,
                    ..Default::default()
                },
            );
        }
        assert_eq!(t.records().len(), 3);
        assert!(t.summary_json(&ctx()).contains("\"frames\":9"));
    }

    #[test]
    fn summary_reports_averages_and_worst_case() {
        let mut t = Telemetry::in_memory();
        for (i, ms) in [10.0f32, 20.0, 30.0].iter().enumerate() {
            t.frame(
                &ctx(),
                &FrameSample {
                    frame: i as u64,
                    frame_ms: *ms,
                    emulated_fps: 59.7275,
                    tx_bytes: 1000,
                    ..Default::default()
                },
            );
        }
        let s = t.summary_json(&ctx());
        assert!(s.contains("\"type\":\"summary\""), "{s}");
        assert!(s.contains("\"frames\":3"));
        assert!(s.contains("\"frame_ms_avg\":20.000"), "{s}");
        assert!(s.contains("\"frame_ms_worst\":30.000"), "{s}");
        assert!(s.contains("\"speed_pct_avg\":100.000"), "{s}");
        assert!(s.contains("\"tx_bytes_avg\":1000.000"), "{s}");
    }

    #[test]
    fn file_sink_open_failure_degrades_to_disabled() {
        let dest = Destination::Path(PathBuf::from("/nonexistent-dir-xyz/t.ndjson"));
        let (t, err) = Telemetry::open(&dest);
        assert!(!t.is_enabled());
        assert!(err.unwrap().contains("cannot open"));
    }
}

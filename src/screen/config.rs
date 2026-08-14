//! Configuration and small persistent state for the terminal frontend.
//!
//! The format is a hand-rolled `key = value` file (a strict subset of TOML, so
//! a real `.toml` file also parses). We deliberately avoid pulling in a config
//! crate: the surface is tiny and the parser below is a dozen lines.
//!
//! Missing config is never an error — [`Config::load`] falls back to
//! [`Config::default`] entirely, and any individual key that is absent or
//! malformed keeps its default. The user config file is only ever *read*; the
//! "recently played" list is the one thing we write back, and it lives in a
//! separate state file so we never rewrite (and lose the comments in) the
//! user's config.

use crate::upscale::Algorithm;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How the game picture should be recoloured before it is drawn. Only affects
/// classic (DMG) games, whose framebuffer is greyscale; colour (CGB) games are
/// passed through untouched. See [`Palette::map`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    /// Pass the greyscale framebuffer through unchanged.
    Grayscale,
    /// The classic olive-green DMG LCD look.
    Green,
    /// A warm amber "pocket" look.
    Amber,
}

impl Palette {
    /// Parse a config value. Accepts a few aliases per palette. (Named like
    /// [`crate::upscale::Algorithm::from_str`], hence not the `FromStr` trait —
    /// the config layer wants `Option`, not `Result`.)
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Palette> {
        match s.trim().to_ascii_lowercase().as_str() {
            "grayscale" | "greyscale" | "gray" | "grey" | "dmg" | "off" => {
                Some(Palette::Grayscale)
            }
            "green" | "lcd" | "gameboy" => Some(Palette::Green),
            "amber" | "orange" | "pocket" => Some(Palette::Amber),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Palette::Grayscale => "grayscale",
            Palette::Green => "green",
            Palette::Amber => "amber",
        }
    }

    /// The next palette in a fixed cycle, for a live toggle key.
    pub fn next(self) -> Palette {
        match self {
            Palette::Grayscale => Palette::Green,
            Palette::Green => Palette::Amber,
            Palette::Amber => Palette::Grayscale,
        }
    }

    /// The four LCD shades, lightest first, as RGB. Index 0 is the "off"/paper
    /// shade (framebuffer value 255) and index 3 the darkest (value 0).
    fn shades(self) -> [[u8; 3]; 4] {
        match self {
            Palette::Grayscale => {
                [[255, 255, 255], [170, 170, 170], [85, 85, 85], [0, 0, 0]]
            }
            // Classic DMG olive-green LCD.
            Palette::Green => {
                [[224, 248, 208], [136, 192, 112], [52, 104, 86], [8, 24, 32]]
            }
            Palette::Amber => {
                [[255, 240, 200], [230, 170, 60], [150, 90, 20], [40, 20, 0]]
            }
        }
    }

    /// One of the four LCD shades by index, lightest first. Game Boy 2bpp
    /// pixels are already palette *indices*, so anything decoding tiles
    /// directly (the trainer view's sprites) wants this rather than
    /// [`Palette::map`], which takes a framebuffer luma.
    pub fn shade(self, index: u8) -> [u8; 3] {
        self.shades()[(index & 3) as usize]
    }

    /// Map one greyscale pixel to this palette. `luma` is the framebuffer byte
    /// (all three channels are equal for DMG output). Returns the pixel
    /// unchanged for [`Palette::Grayscale`].
    pub fn map(self, luma: u8) -> [u8; 3] {
        if self == Palette::Grayscale {
            return [luma, luma, luma];
        }
        // Framebuffer emits exactly 255 / 192 / 96 / 0 (see gpu::mod
        // get_monochrome_pal_val). Bucket by nearest to be robust anyway.
        let shade = if luma >= 224 {
            0
        } else if luma >= 144 {
            1
        } else if luma >= 48 {
            2
        } else {
            3
        };
        self.shades()[shade]
    }
}

/// User-configurable Game Boy button bindings (single characters, matched
/// case-insensitively). Movement is always the arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBindings {
    pub a: char,
    pub b: char,
    pub start: char,
    pub select: char,
}

impl Default for KeyBindings {
    fn default() -> Self {
        KeyBindings {
            a: 'a',
            b: 's',
            start: 'x',
            select: 'z',
        }
    }
}

/// How many *device* pixels one terminal cell really is, relative to the cell
/// size the terminal reports over its escape queries.
///
/// This exists because of a HiDPI mismatch specific to pixel-addressed image
/// protocols (iTerm2/sixel) on scaled Wayland: the terminal answers the
/// cell-size query (`CSI 16 t`) in **logical** pixels, but renders an inline
/// image sized in **physical** device pixels. On a display scaled by *S*, an
/// image sized to the logical cell grid therefore comes out *S* times too small
/// — the "tiny picture" bug. Multiplying the cell size the frontend feeds to
/// `ratatui-image` by *S* makes the emitted image physical-pixel sized again, so
/// it fills the panel. See [`crate::screen::tui`] for where it is applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellScale {
    /// Detect the factor at runtime (env / X resources); falls back to 1.0.
    Auto,
    /// A fixed multiplier. `1.0` is "trust the terminal" (the safe default).
    Fixed(f32),
}

impl Default for CellScale {
    fn default() -> Self {
        CellScale::Fixed(1.0)
    }
}

impl CellScale {
    /// Parse a config value: `auto`, or a positive number. Out-of-range or
    /// unparseable values fall back to the default (1.0), never an error.
    fn from_str(s: &str) -> CellScale {
        let s = s.trim();
        if s.eq_ignore_ascii_case("auto") {
            return CellScale::Auto;
        }
        match s.parse::<f32>() {
            // Clamp to a sane band: below 1 would shrink, and nothing needs more
            // than 8x. A non-finite or <=0 value is meaningless → default.
            Ok(v) if v.is_finite() && v > 0.0 => CellScale::Fixed(v.clamp(1.0, 8.0)),
            _ => CellScale::default(),
        }
    }
}

/// How big an image the frontend sends to the terminal, relative to the
/// 160×144 framebuffer.
///
/// **This is the knob that stops render cost scaling with window size.** The
/// terminal scales whatever it is sent to the display rectangle, so sending a
/// window-sized image is pure waste: measured at a 1568×1408 rectangle it costs
/// ~41 ms and 11.6 MB *per frame* (≈20 fps), against ~0.1 ms and 90 KB for the
/// 160×144 original. See [`crate::termimage`] and `examples/render_bench.rs`.
///
/// `Auto` sends:
/// * `nearest` → **×1**. Integer pixel duplication is exactly what the terminal
///   does when it stretches the image, so doing it first buys nothing and costs
///   16× the bytes.
/// * a real filter (`scale2x`/`mmpx`/`lcd`) → **its own factor**, because there
///   the upscale *is* the visual effect and must happen before transmission —
///   but never more than the algorithm's own factor, and never the window size.
///
/// A fixed value overrides that, for a terminal whose own scaling is too soft
/// for taste. Bigger is sharper under a smoothing terminal and linearly more
/// bytes per frame; `2` is the usual answer, `4` is 1.4 MB/frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransmitScale {
    #[default]
    Auto,
    Fixed(u8),
}

impl TransmitScale {
    /// The factor to actually upscale by before transmitting, given the
    /// configured upscaler.
    pub fn factor_for(self, algo: Algorithm, factor: u8) -> u8 {
        match self {
            TransmitScale::Auto => match algo {
                Algorithm::Nearest => 1,
                _ => factor,
            },
            // A fixed request still has to be a factor the algorithm can
            // produce (mmpx has no ×3).
            TransmitScale::Fixed(n) => algo.clamp_factor(n.max(1)),
        }
    }

    fn parse(s: &str) -> Option<TransmitScale> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("auto") {
            return Some(TransmitScale::Auto);
        }
        match s.parse::<u8>() {
            Ok(n) if (1..=6).contains(&n) => Some(TransmitScale::Fixed(n)),
            _ => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            TransmitScale::Auto => "auto".to_string(),
            TransmitScale::Fixed(n) => format!("x{n}"),
        }
    }
}

/// Which terminal render path to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderPath {
    /// Transmit at a capped resolution and let the terminal scale (default).
    #[default]
    Fast,
    /// The pre-2026 path: `ratatui-image` resizes the frame to the display
    /// rectangle and re-encodes it every frame. Kept as an escape hatch for a
    /// terminal that mis-renders cell-scaled images; it is 20–400× more
    /// expensive on a large window.
    Legacy,
}

impl RenderPath {
    fn parse(s: &str) -> Option<RenderPath> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" | "capped" | "default" => Some(RenderPath::Fast),
            "legacy" | "old" | "resize" => Some(RenderPath::Legacy),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RenderPath::Fast => "fast",
            RenderPath::Legacy => "legacy",
        }
    }
}

/// How loudly to announce a wild encounter's shiny-if-traded verdict.
///
/// The banner exists to be impossible to miss, so "on" is the default and
/// includes the terminal bell — but a bell is exactly the sort of thing a person
/// wants to be able to switch off, and someone watching a stream may want the
/// row gone entirely. See [`crate::trainer::encounter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShinyAlert {
    /// Banner for every catchable wild encounter, plus a toast and the terminal
    /// bell when one would be shiny in Gen 2 (default).
    #[default]
    On,
    /// The same, without the bell.
    Quiet,
    /// Nothing. The verdict is still in the trainer panel's battle section.
    Off,
}

impl ShinyAlert {
    fn parse(s: &str) -> Option<ShinyAlert> {
        match s.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" | "bell" => Some(ShinyAlert::On),
            "quiet" | "silent" | "nobell" => Some(ShinyAlert::Quiet),
            "off" | "false" | "no" | "0" => Some(ShinyAlert::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ShinyAlert::On => "on",
            ShinyAlert::Quiet => "quiet",
            ShinyAlert::Off => "off",
        }
    }

    /// Whether the encounter row is drawn at all.
    pub fn shows_banner(self) -> bool {
        self != ShinyAlert::Off
    }

    /// Whether a shiny verdict rings the terminal bell.
    pub fn rings(self) -> bool {
        self == ShinyAlert::On
    }
}

/// The parsed terminal-frontend configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Directory the ROM picker browses by default.
    pub rom_dir: PathBuf,
    /// Preferred image protocol, or `None` to auto-detect the best available.
    pub protocol: Option<String>,
    pub palette: Palette,
    /// Pixel-art upscaler run on every frame before display. `nearest` at
    /// factor 1 (the default) is a no-op. See [`crate::upscale`].
    pub upscale: Algorithm,
    pub upscale_factor: u8,
    pub keys: KeyBindings,
    /// Device-pixels-per-cell correction for HiDPI pixel protocols. See
    /// [`CellScale`]. Default `Fixed(1.0)` = no correction (never regresses a
    /// low-DPI or already-physical terminal).
    pub cell_scale: CellScale,
    /// How large an image is sent to the terminal. See [`TransmitScale`].
    pub transmit_scale: TransmitScale,
    /// Which render path the game picture uses. See [`RenderPath`].
    pub render_path: RenderPath,
    /// How loudly a wild Pokémon's shiny-if-traded verdict is announced. See
    /// [`ShinyAlert`].
    pub shiny_alert: ShinyAlert,
    /// Where to append NDJSON run telemetry, or `None` (the default) for off.
    /// See [`crate::telemetry`].
    pub telemetry: Option<String>,
    /// Where the `c` key writes PNGs. `None` (the default) means
    /// `$XDG_PICTURES_DIR/gameboy`, else `~/Pictures/gameboy`.
    pub screenshot_dir: Option<String>,
    /// Where the `v` key writes recordings when no `video_out` destination is
    /// configured. `None` means `$XDG_VIDEOS_DIR/gameboy`, else `~/Videos/gameboy`.
    pub video_dir: Option<String>,
    /// The v4l2loopback node the user is feeding through `ffmpeg`, if any.
    /// Used *only* to check that the camera's label names the running game —
    /// the emulator never writes to a V4L2 node itself. See
    /// [`crate::video::v4l2`].
    pub video_device: Option<String>,
    /// Raw video-out settings, carried as written and parsed by
    /// [`crate::video::VideoSpec::resolve`] so the config file and the
    /// `GB_VIDEO_*` environment variables cannot drift apart.
    pub video: crate::video::RawSettings,
    /// Plugins to attach, by name (`gen1-link`). Empty by default: **nothing
    /// loads implicitly**, so an unmodified `config.toml` runs an unmodified
    /// machine. See [`crate::plugin`].
    pub plugins: Vec<String>,
    /// Trade file for the Gen 1 link partner, or `None` for its built-in offer.
    /// See [`crate::plugin::gen1::spec`].
    pub gen1_trade: Option<String>,
    /// Battle-preset file for the Gen 1 battle plugin, or `None` for its
    /// built-in presets. See [`crate::plugin::battle::preset`].
    pub battles: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            rom_dir: default_rom_dir(),
            protocol: None,
            palette: Palette::Grayscale,
            upscale: Algorithm::Nearest,
            upscale_factor: 1,
            keys: KeyBindings::default(),
            cell_scale: CellScale::default(),
            transmit_scale: TransmitScale::default(),
            render_path: RenderPath::default(),
            shiny_alert: ShinyAlert::default(),
            telemetry: None,
            screenshot_dir: None,
            video_dir: None,
            video_device: None,
            video: crate::video::RawSettings::default(),
            plugins: Vec::new(),
            gen1_trade: None,
            battles: None,
        }
    }
}

/// `~/roms`, or `./roms` if `$HOME` is somehow unset.
pub fn default_rom_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => Path::new(&home).join("roms"),
        None => PathBuf::from("roms"),
    }
}

/// `${XDG_CONFIG_HOME:-~/.config}/gameboy/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gameboy").join("config.toml"))
}

/// `${XDG_STATE_HOME:-~/.local/state}/gameboy/recent`.
pub fn recent_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".local").join("state"))
        })?;
    Some(base.join("gameboy").join("recent"))
}

impl Config {
    /// Load the config file if present, otherwise return defaults. Never fails:
    /// an unreadable or partially malformed file degrades to defaults per-key.
    pub fn load() -> Config {
        Config::load_reporting().0
    }

    /// [`Config::load`] plus every complaint the parser has about the file.
    ///
    /// A config key that does not apply used to fail **silently** — `palette =
    /// 'green'` (a perfectly good TOML literal string) left the picture
    /// grayscale with nothing said, while the numeric `upscale_factor = 4` next
    /// to it worked, which is a genuinely maddening way to lose an afternoon.
    /// Values are now accepted single-quoted as well as double-quoted or bare,
    /// and anything still unusable is *reported* rather than dropped.
    pub fn load_reporting() -> (Config, Vec<String>) {
        match config_path().filter(|p| p.exists()) {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(text) => Config::parse_reporting(&text),
                Err(e) => (
                    Config::default(),
                    vec![format!("config: cannot read {}: {e}", path.display())],
                ),
            },
            None => (Config::default(), Vec::new()),
        }
    }

    /// Parse a `key = value` document into a [`Config`], filling defaults for
    /// anything missing or unrecognised.
    pub fn parse(text: &str) -> Config {
        Config::parse_reporting(text).0
    }

    /// [`Config::parse`], also returning a human-readable complaint for every
    /// key that was not understood or whose value could not be applied.
    pub fn parse_reporting(text: &str) -> (Config, Vec<String>) {
        let kv = parse_kv(text);
        let mut cfg = Config::default();
        let mut warn: Vec<String> = Vec::new();
        // Tracks which keys were consumed, so anything left over is reported as
        // an unknown key (a typo silently doing nothing is the same bug class).
        let mut seen: Vec<&str> = Vec::new();
        let mut get = |key: &'static str| -> Option<&String> {
            seen.push(key);
            kv.get(key)
        };

        if let Some(v) = get("rom_dir") {
            cfg.rom_dir = expand_tilde(v);
        }
        if let Some(v) = get("protocol") {
            let v = v.trim();
            cfg.protocol = if v.is_empty() || v.eq_ignore_ascii_case("auto") {
                None
            } else {
                Some(v.to_string())
            };
        }
        if let Some(v) = get("palette") {
            match Palette::from_str(v) {
                Some(p) => cfg.palette = p,
                None => warn.push(bad("palette", v, "grayscale, green, amber")),
            }
        }
        if let Some(v) = get("upscale") {
            match Algorithm::from_str(v) {
                Some(a) => {
                    cfg.upscale = a;
                    // An algorithm without an explicit factor gets that
                    // algorithm's smallest useful factor rather than staying 1.
                    cfg.upscale_factor = a.clamp_factor(cfg.upscale_factor);
                }
                None => warn.push(bad("upscale", v, "nearest, scale2x, mmpx, lcd")),
            }
        }
        if let Some(v) = get("upscale_factor") {
            match v.trim().parse::<u8>() {
                Ok(f) => cfg.upscale_factor = cfg.upscale.clamp_factor(f),
                Err(_) => warn.push(bad("upscale_factor", v, "a number 1-6")),
            }
        }
        for key in ["key_a", "key_b", "key_start", "key_select"] {
            let Some(v) = get(key) else { continue };
            let Some(c) = first_char(v) else {
                warn.push(bad(key, v, "a single character"));
                continue;
            };
            match key {
                "key_a" => cfg.keys.a = c,
                "key_b" => cfg.keys.b = c,
                "key_start" => cfg.keys.start = c,
                _ => cfg.keys.select = c,
            }
        }
        if let Some(v) = get("cell_scale") {
            let parsed = CellScale::from_str(v);
            if parsed == CellScale::default()
                && !v.trim().eq_ignore_ascii_case("1")
                && !v.trim().eq_ignore_ascii_case("1.0")
            {
                warn.push(bad("cell_scale", v, "auto, or a number 1.0-8.0"));
            }
            cfg.cell_scale = parsed;
        }
        if let Some(v) = get("transmit_scale") {
            match TransmitScale::parse(v) {
                Some(t) => cfg.transmit_scale = t,
                None => warn.push(bad("transmit_scale", v, "auto, or a number 1-6")),
            }
        }
        if let Some(v) = get("render_path") {
            match RenderPath::parse(v) {
                Some(p) => cfg.render_path = p,
                None => warn.push(bad("render_path", v, "fast, legacy")),
            }
        }
        if let Some(v) = get("shiny_alert") {
            match ShinyAlert::parse(v) {
                Some(a) => cfg.shiny_alert = a,
                None => warn.push(bad("shiny_alert", v, "on, quiet, off")),
            }
        }
        if let Some(v) = get("telemetry") {
            let v = v.trim();
            cfg.telemetry = if v.is_empty()
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("false")
                || v == "0"
            {
                None
            } else {
                Some(expand_tilde_str(v))
            };
        }

        for key in ["screenshot_dir", "video_dir", "video_device"] {
            let Some(v) = get(key) else { continue };
            let v = v.trim();
            let slot = match key {
                "screenshot_dir" => &mut cfg.screenshot_dir,
                "video_dir" => &mut cfg.video_dir,
                _ => &mut cfg.video_device,
            };
            *slot = if v.is_empty() {
                None
            } else {
                Some(expand_tilde_str(v))
            };
        }

        // Video-out. The values are carried through as written: `video.rs`
        // owns their parsing (and their complaints), so the config file and
        // the `GB_VIDEO_*` environment variables cannot disagree about what
        // `yuyv` means. `video_out = off` is the default and is not an error.
        for key in [
            "video_out",
            "video_format",
            "video_size",
            "video_source",
            "video_pace",
            // Naming a destination is not the same decision as sending frames
            // to it: `video_out` says *where* `v` puts the stream, this says
            // "and go live at launch". Default off — see
            // [`crate::video::resolve_autostart`].
            "video_autostart",
        ] {
            let Some(v) = get(key) else { continue };
            let v = v.trim();
            let slot = match key {
                "video_out" => &mut cfg.video.out,
                "video_format" => &mut cfg.video.format,
                "video_size" => &mut cfg.video.size,
                "video_source" => &mut cfg.video.source,
                "video_autostart" => &mut cfg.video.autostart,
                _ => &mut cfg.video.pace,
            };
            // `video_out` is a path (or `-`, or `off`); the rest are keywords,
            // and a keyword never starts with `~`, so one rule covers both.
            *slot = if v.is_empty() {
                None
            } else {
                Some(expand_tilde_str(v))
            };
        }

        // Plugins. Listed by name, comma-separated; unknown names are reported
        // by the frontend when it tries to attach them, not here, so the
        // config parser stays a parser.
        if let Some(v) = get("plugins") {
            let v = v.trim();
            cfg.plugins = if v.is_empty()
                || v.eq_ignore_ascii_case("off")
                || v.eq_ignore_ascii_case("none")
                || v == "0"
            {
                Vec::new()
            } else {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
        }
        if let Some(v) = get("gen1_trade") {
            let v = v.trim();
            cfg.gen1_trade = if v.is_empty() || v.eq_ignore_ascii_case("off") {
                None
            } else {
                Some(expand_tilde_str(v))
            };
        }
        if let Some(v) = get("battles") {
            let v = v.trim();
            cfg.battles = if v.is_empty() || v.eq_ignore_ascii_case("off") {
                None
            } else {
                Some(v.to_string())
            };
        }

        let mut unknown: Vec<&String> =
            kv.keys().filter(|k| !seen.contains(&k.as_str())).collect();
        unknown.sort();
        for k in unknown {
            warn.push(format!("config: unknown key `{k}` (ignored)"));
        }
        warn.sort();
        (cfg, warn)
    }
}

fn bad(key: &str, value: &str, expected: &str) -> String {
    format!("config: `{key} = {value}` not understood (expected {expected}) — using the default")
}

/// Parse `key = value` lines into a map. `#` starts a comment; values may be
/// bare, `"double-quoted"` or `'single-quoted'`; blank lines and unknown junk
/// are ignored.
fn parse_kv(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw in text.lines() {
        let line = match raw.split_once('#') {
            Some((before, _)) => before,
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = unquote(value.trim());
        if !key.is_empty() {
            map.insert(key, value.to_string());
        }
    }
    map
}

/// Strip a matching pair of surrounding quotes. TOML has both basic (`"…"`) and
/// literal (`'…'`) strings and `config.toml` invites both; only handling one
/// was the silent-config bug.
fn unquote(value: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    value
}

fn first_char(s: &str) -> Option<char> {
    s.trim().chars().next()
}

/// [`expand_tilde`] for the keys carried as strings rather than as `PathBuf`.
///
/// **Every path-valued key goes through one of these two.** `rom_dir` was
/// expanded and nothing else was, so the `~/mewtwo.trade` written in
/// `docs/plugins.md` failed with "cannot read ~/mewtwo.trade" — a shell expands
/// `~` before the program ever sees it, so a path from a *config file* has to be
/// expanded by the program or it silently only works when typed at a prompt.
fn expand_tilde_str(s: &str) -> String {
    expand_tilde(s).to_string_lossy().into_owned()
}

/// Expand a leading `~` or `~/` to `$HOME`.
fn expand_tilde(s: &str) -> PathBuf {
    let s = s.trim();
    if s == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(s)
}

/// The recently-played ROM list, most-recent first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recent {
    pub paths: Vec<PathBuf>,
}

const RECENT_MAX: usize = 12;

impl Recent {
    /// Load the recent list, dropping any entries that no longer exist. Never
    /// fails.
    pub fn load() -> Recent {
        let Some(path) = recent_path().filter(|p| p.exists()) else {
            return Recent::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Recent::parse(&text),
            Err(_) => Recent::default(),
        }
    }

    pub fn parse(text: &str) -> Recent {
        let paths = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();
        Recent { paths }
    }

    fn serialize(&self) -> String {
        let mut out = String::new();
        for p in &self.paths {
            out.push_str(&p.to_string_lossy());
            out.push('\n');
        }
        out
    }

    /// Record `path` as the most recently played, de-duplicating and capping the
    /// list length. Pure — see [`Recent::save`] to persist.
    pub fn record(&mut self, path: &Path) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.paths.retain(|p| p != &canonical && p != path);
        self.paths.insert(0, canonical);
        self.paths.truncate(RECENT_MAX);
    }

    /// Persist the list to the state file (best-effort; errors are ignored).
    pub fn save(&self) {
        let Some(path) = recent_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, self.serialize());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.protocol, None);
        assert_eq!(cfg.palette, Palette::Grayscale);
        assert_eq!(cfg.keys.a, 'a');
        assert!(cfg.rom_dir.ends_with("roms"));
    }

    #[test]
    fn empty_document_is_defaults() {
        assert_eq!(Config::parse(""), Config::default());
        assert_eq!(Config::parse("\n\n   \n"), Config::default());
    }

    /// `~` in a config file is not expanded by anything else — there is no
    /// shell between the file and the program — so **every** path-valued key
    /// has to expand it. Only `rom_dir` did, which is why the `~/mewtwo.trade`
    /// in `docs/plugins.md` failed with "cannot read ~/mewtwo.trade".
    #[test]
    fn every_path_key_expands_a_leading_tilde() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return; // no HOME on this box; the expansion is a no-op by design
        };
        let cfg = Config::parse(
            "rom_dir = ~/roms\n\
             gen1_trade = ~/mewtwo.trade\n\
             telemetry = ~/gb.ndjson\n\
             screenshot_dir = ~/Pictures/gb\n\
             video_dir = ~/Videos/gb\n\
             video_out = ~/out.raw\n",
        );
        let want = |rel: &str| home.join(rel).to_string_lossy().into_owned();
        assert_eq!(cfg.rom_dir, home.join("roms"));
        assert_eq!(
            cfg.gen1_trade.as_deref(),
            Some(want("mewtwo.trade").as_str())
        );
        assert_eq!(cfg.telemetry.as_deref(), Some(want("gb.ndjson").as_str()));
        assert_eq!(
            cfg.screenshot_dir.as_deref(),
            Some(want("Pictures/gb").as_str())
        );
        assert_eq!(cfg.video_dir.as_deref(), Some(want("Videos/gb").as_str()));
        assert_eq!(cfg.video.out.as_deref(), Some(want("out.raw").as_str()));
        // A keyword value is not a path and must come through untouched.
        assert_eq!(
            Config::parse("video_out = -\nvideo_pace = realtime")
                .video
                .out
                .as_deref(),
            Some("-")
        );
    }

    #[test]
    fn parses_all_fields() {
        let doc = r#"
            # my config
            rom_dir = "/games/gb"
            protocol = kitty
            palette = green
            key_a = j      # comment after value
            key_b = k
            key_start = "n"
            key_select = m
        "#;
        let cfg = Config::parse(doc);
        assert_eq!(cfg.rom_dir, PathBuf::from("/games/gb"));
        assert_eq!(cfg.protocol, Some("kitty".to_string()));
        assert_eq!(cfg.palette, Palette::Green);
        assert_eq!(cfg.keys.a, 'j');
        assert_eq!(cfg.keys.b, 'k');
        assert_eq!(cfg.keys.start, 'n');
        assert_eq!(cfg.keys.select, 'm');
    }

    /// The battle plugin's preset file, alongside the link partner's trade
    /// file. A key that is not read by `parse_reporting`'s `get(...)` list is
    /// reported as *unknown*, so a new key that is only added to the struct
    /// looks broken to the user — this pins both halves.
    #[test]
    fn the_battle_preset_file_is_configurable_and_optional() {
        assert_eq!(Config::default().battles, None);
        let c = Config::parse("plugins = gen1-battle\nbattles = '/tmp/b.preset'\n");
        assert_eq!(c.plugins, vec!["gen1-battle".to_string()]);
        assert_eq!(c.battles.as_deref(), Some("/tmp/b.preset"));
        assert_eq!(Config::parse("battles = off\n").battles, None);
        let (_, warn) =
            Config::parse_reporting("plugins = gen1-battle\nbattles = /tmp/b\n");
        assert!(warn.is_empty(), "{warn:?}");
    }

    #[test]
    fn cell_scale_defaults_to_one() {
        assert_eq!(Config::default().cell_scale, CellScale::Fixed(1.0));
        assert_eq!(Config::parse("").cell_scale, CellScale::Fixed(1.0));
    }

    #[test]
    fn cell_scale_parses_float_auto_and_clamps() {
        assert_eq!(
            Config::parse("cell_scale = 2.0").cell_scale,
            CellScale::Fixed(2.0)
        );
        assert_eq!(
            Config::parse("cell_scale = 2").cell_scale,
            CellScale::Fixed(2.0)
        );
        assert_eq!(
            Config::parse("cell_scale = auto").cell_scale,
            CellScale::Auto
        );
        assert_eq!(
            Config::parse("cell_scale = AUTO").cell_scale,
            CellScale::Auto
        );
        // Out of band / nonsense → safe default 1.0.
        assert_eq!(
            Config::parse("cell_scale = 0").cell_scale,
            CellScale::Fixed(1.0)
        );
        assert_eq!(
            Config::parse("cell_scale = -3").cell_scale,
            CellScale::Fixed(1.0)
        );
        assert_eq!(
            Config::parse("cell_scale = 99").cell_scale,
            CellScale::Fixed(8.0)
        );
        assert_eq!(
            Config::parse("cell_scale = nope").cell_scale,
            CellScale::Fixed(1.0)
        );
    }

    #[test]
    fn protocol_auto_means_none() {
        assert_eq!(Config::parse("protocol = auto").protocol, None);
        assert_eq!(Config::parse("protocol =   ").protocol, None);
    }

    #[test]
    fn unknown_keys_and_bad_palette_ignored() {
        let cfg = Config::parse("wobble = 3\npalette = tartan\n");
        assert_eq!(cfg.palette, Palette::Grayscale);
        assert_eq!(cfg, Config::default());
    }

    // -- The silent-config bug -------------------------------------------
    //
    // `config.toml` invites TOML syntax, and TOML has *two* string forms.
    // Single-quoted values were silently dropped: `palette = 'green'` left the
    // picture grayscale while the numeric `upscale_factor = 4` beside it
    // applied, which looks exactly like "the feature is broken".

    #[test]
    fn single_quoted_values_apply() {
        let cfg =
            Config::parse("palette = 'green'\nupscale = 'lcd'\nupscale_factor = 4\n");
        assert_eq!(cfg.palette, Palette::Green);
        assert_eq!(cfg.upscale, Algorithm::LcdGrid);
        assert_eq!(cfg.upscale_factor, 4);
    }

    #[test]
    fn every_quoting_style_agrees() {
        for form in ["green", "\"green\"", "'green'", "  green  "] {
            assert_eq!(
                Config::parse(&format!("palette = {form}")).palette,
                Palette::Green,
                "{form}"
            );
        }
    }

    #[test]
    fn a_value_that_does_not_apply_is_reported() {
        let (cfg, warnings) = Config::parse_reporting("palette = tartan\n");
        assert_eq!(cfg.palette, Palette::Grayscale);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("palette"), "{warnings:?}");
        assert!(warnings[0].contains("tartan"), "{warnings:?}");
        assert!(
            warnings[0].contains("green"),
            "should list what is accepted"
        );
    }

    #[test]
    fn a_mistyped_key_is_reported() {
        let (_, warnings) = Config::parse_reporting("pallete = green\n");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("unknown key `pallete`"),
            "{warnings:?}"
        );
    }

    #[test]
    fn every_bad_value_is_reported_not_just_the_first() {
        let (_, warnings) = Config::parse_reporting(
            "palette = tartan\nupscale = hqx\nupscale_factor = banana\nrender_path = sideways\n",
        );
        assert_eq!(warnings.len(), 4, "{warnings:?}");
    }

    #[test]
    fn a_good_config_warns_about_nothing() {
        let (_, warnings) = Config::parse_reporting(
            "# comment\nrom_dir = \"/games\"\npalette = green\nupscale = mmpx\n\
             upscale_factor = 2\nprotocol = kitty\ncell_scale = auto\n\
             transmit_scale = auto\nrender_path = fast\ntelemetry = /tmp/gb.ndjson\n\
             key_a = j\nkey_b = k\nkey_start = n\nkey_select = m\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn transmit_scale_parses_and_clamps() {
        assert_eq!(Config::default().transmit_scale, TransmitScale::Auto);
        assert_eq!(
            Config::parse("transmit_scale = 2").transmit_scale,
            TransmitScale::Fixed(2)
        );
        assert_eq!(
            Config::parse("transmit_scale = auto").transmit_scale,
            TransmitScale::Auto
        );
        // Out of range falls back to the default *and* says so.
        let (cfg, warnings) = Config::parse_reporting("transmit_scale = 99");
        assert_eq!(cfg.transmit_scale, TransmitScale::Auto);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn render_path_and_telemetry_parse() {
        assert_eq!(Config::default().render_path, RenderPath::Fast);
        assert_eq!(
            Config::parse("render_path = legacy").render_path,
            RenderPath::Legacy
        );
        assert_eq!(
            Config::parse("telemetry = '/tmp/gb.ndjson'")
                .telemetry
                .as_deref(),
            Some("/tmp/gb.ndjson")
        );
        assert_eq!(Config::parse("telemetry = off").telemetry, None);
        assert_eq!(Config::default().telemetry, None);
    }

    #[test]
    fn video_out_keys_are_carried_through_for_video_rs_to_parse() {
        assert_eq!(
            Config::default().video,
            crate::video::RawSettings::default()
        );
        let (cfg, warnings) = Config::parse_reporting(
            "video_out = /tmp/gb.raw\nvideo_format = yuyv\nvideo_size = 640x480\n\
             video_source = display\nvideo_pace = realtime\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.video.out.as_deref(), Some("/tmp/gb.raw"));
        assert_eq!(cfg.video.format.as_deref(), Some("yuyv"));
        assert_eq!(cfg.video.size.as_deref(), Some("640x480"));
        assert_eq!(cfg.video.source.as_deref(), Some("display"));
        assert_eq!(cfg.video.pace.as_deref(), Some("realtime"));
    }

    #[test]
    fn no_plugins_are_configured_by_default() {
        // The safety property from docs/plugins.md: nothing attaches unless the
        // user asked for it by name.
        assert!(Config::default().plugins.is_empty());
        assert!(Config::parse("palette = green").plugins.is_empty());
        assert_eq!(Config::default().gen1_trade, None);
    }

    #[test]
    fn plugins_and_trade_file_parse() {
        let c = Config::parse("plugins = gen1-link\ngen1_trade = '/tmp/mew.txt'\n");
        assert_eq!(c.plugins, vec!["gen1-link".to_string()]);
        assert_eq!(c.gen1_trade.as_deref(), Some("/tmp/mew.txt"));
        // A list, bare or quoted, with whitespace tolerated.
        assert_eq!(
            Config::parse("plugins = gen1-link, trainer-view").plugins,
            vec!["gen1-link".to_string(), "trainer-view".to_string()]
        );
        // And the explicit off switches.
        assert!(Config::parse("plugins = none").plugins.is_empty());
        assert!(Config::parse("plugins =").plugins.is_empty());
    }

    #[test]
    fn the_plugin_keys_are_not_reported_as_unknown() {
        // Every config key must be in the `get(...)` list or the user is told
        // their setting is unknown — the bug docs/plugins.md's §4 relies on not
        // having.
        let (_, warnings) =
            Config::parse_reporting("plugins = gen1-link\ngen1_trade = /tmp/x\n");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn malformed_lines_do_not_crash() {
        // No '=', stray quotes, empty key — all skipped.
        let cfg = Config::parse("just some words\n= value\n\"quoted\"\n");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn tilde_expands_when_home_set() {
        // expand_tilde reads $HOME; assert relative to whatever it is.
        if let Some(home) = std::env::var_os("HOME") {
            let cfg = Config::parse("rom_dir = ~/games");
            assert_eq!(cfg.rom_dir, PathBuf::from(home).join("games"));
        }
    }

    #[test]
    fn upscale_keys_parse_and_clamp() {
        // Explicit pair.
        let cfg = Config::parse("upscale = mmpx\nupscale_factor = 2\n");
        assert_eq!(cfg.upscale, Algorithm::Mmpx);
        assert_eq!(cfg.upscale_factor, 2);
        // Algorithm without a factor gets its smallest useful factor.
        let cfg = Config::parse("upscale = scale2x\n");
        assert_eq!(cfg.upscale, Algorithm::Scale2x);
        assert_eq!(cfg.upscale_factor, 2);
        // Factor is clamped to what the algorithm supports.
        let cfg = Config::parse("upscale = mmpx\nupscale_factor = 9\n");
        assert_eq!(cfg.upscale_factor, 4);
        // Unknown algorithm and junk factors fall back to defaults.
        let cfg = Config::parse("upscale = hqx\nupscale_factor = banana\n");
        assert_eq!(cfg.upscale, Algorithm::Nearest);
        assert_eq!(cfg.upscale_factor, 1);
        // Default is a no-op.
        assert_eq!(Config::default().upscale, Algorithm::Nearest);
        assert_eq!(Config::default().upscale_factor, 1);
    }

    #[test]
    fn palette_grayscale_is_identity() {
        for v in [0u8, 96, 192, 255, 137] {
            assert_eq!(Palette::Grayscale.map(v), [v, v, v]);
        }
    }

    #[test]
    fn palette_green_maps_four_shades() {
        // The four exact framebuffer values must land on the four shades,
        // lightest-to-darkest.
        let g = Palette::Green;
        assert_eq!(g.map(255), [224, 248, 208]);
        assert_eq!(g.map(192), [136, 192, 112]);
        assert_eq!(g.map(96), [52, 104, 86]);
        assert_eq!(g.map(0), [8, 24, 32]);
    }

    #[test]
    fn palette_cycle_is_stable() {
        let mut p = Palette::Grayscale;
        for _ in 0..3 {
            p = p.next();
        }
        assert_eq!(p, Palette::Grayscale);
    }

    #[test]
    fn recent_records_dedup_and_caps() {
        let mut r = Recent::default();
        // Use paths that won't canonicalize (non-existent) so we test the
        // fallback branch deterministically.
        for i in 0..20 {
            r.record(Path::new(&format!("/roms/game{i}.gb")));
        }
        assert_eq!(r.paths.len(), RECENT_MAX);
        // Most recent first.
        assert_eq!(r.paths[0], PathBuf::from("/roms/game19.gb"));
    }

    #[test]
    fn recent_record_moves_existing_to_front() {
        let mut r = Recent::default();
        r.record(Path::new("/roms/a.gb"));
        r.record(Path::new("/roms/b.gb"));
        r.record(Path::new("/roms/a.gb"));
        assert_eq!(r.paths[0], PathBuf::from("/roms/a.gb"));
        assert_eq!(r.paths.len(), 2);
    }

    #[test]
    fn recent_roundtrips_through_text() {
        let mut r = Recent::default();
        r.record(Path::new("/roms/b.gb"));
        r.record(Path::new("/roms/a.gb"));
        let text = r.serialize();
        assert_eq!(Recent::parse(&text), r);
    }
}

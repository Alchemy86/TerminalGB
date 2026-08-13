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

/// The parsed terminal-frontend configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Directory the ROM picker browses by default.
    pub rom_dir: PathBuf,
    /// Preferred image protocol, or `None` to auto-detect the best available.
    pub protocol: Option<String>,
    pub palette: Palette,
    pub keys: KeyBindings,
    /// Device-pixels-per-cell correction for HiDPI pixel protocols. See
    /// [`CellScale`]. Default `Fixed(1.0)` = no correction (never regresses a
    /// low-DPI or already-physical terminal).
    pub cell_scale: CellScale,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            rom_dir: default_rom_dir(),
            protocol: None,
            palette: Palette::Grayscale,
            keys: KeyBindings::default(),
            cell_scale: CellScale::default(),
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
        match config_path().filter(|p| p.exists()) {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(text) => Config::parse(&text),
                Err(_) => Config::default(),
            },
            None => Config::default(),
        }
    }

    /// Parse a `key = value` document into a [`Config`], filling defaults for
    /// anything missing or unrecognised.
    pub fn parse(text: &str) -> Config {
        let kv = parse_kv(text);
        let mut cfg = Config::default();

        if let Some(v) = kv.get("rom_dir") {
            cfg.rom_dir = expand_tilde(v);
        }
        if let Some(v) = kv.get("protocol") {
            let v = v.trim();
            cfg.protocol = if v.is_empty() || v.eq_ignore_ascii_case("auto") {
                None
            } else {
                Some(v.to_string())
            };
        }
        if let Some(v) = kv.get("palette") {
            if let Some(p) = Palette::from_str(v) {
                cfg.palette = p;
            }
        }
        if let Some(c) = kv.get("key_a").and_then(|v| first_char(v)) {
            cfg.keys.a = c;
        }
        if let Some(c) = kv.get("key_b").and_then(|v| first_char(v)) {
            cfg.keys.b = c;
        }
        if let Some(c) = kv.get("key_start").and_then(|v| first_char(v)) {
            cfg.keys.start = c;
        }
        if let Some(c) = kv.get("key_select").and_then(|v| first_char(v)) {
            cfg.keys.select = c;
        }
        if let Some(v) = kv.get("cell_scale") {
            cfg.cell_scale = CellScale::from_str(v);
        }
        cfg
    }
}

/// Parse `key = value` lines into a map. `#` starts a comment; values may be
/// bare or double-quoted; blank lines and unknown junk are ignored.
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
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        if !key.is_empty() {
            map.insert(key, value.to_string());
        }
    }
    map
}

fn first_char(s: &str) -> Option<char> {
    s.trim().chars().next()
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

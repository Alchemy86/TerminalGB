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
use crate::screen::config::{Config, Palette, Recent};
use crate::screen::romlib::{self, RomEntry};

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

/// Wall-clock time one emulated frame occupies. The DMG refreshes at
/// 59.7275 Hz (4194304 Hz / 70224 cycles-per-frame), **not** a rounded 60 —
/// pacing at 16ms would run ~4.6% fast (62.5 fps), the same bug the audio crew
/// found on the desktop path. 1/59.7275 s ≈ 16743 µs. Emulation of a single
/// frame runs on the main loop; input is polled between frames.
const FRAME_TIME: Duration = Duration::from_micros(16_743);
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
/// than real time produces more samples than the host drains — the cpal queue
/// ([`init_audio`], bounded to ~180 ms by the audio crew) discards the oldest,
/// i.e. **fast-forward drops samples rather than pitch-shifting**; slow-motion
/// under-runs into brief silence. No resampling, by choice.
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

    let config = Config::load();
    let recent = Recent::load();

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

    let mut app = App::new(config, recent);
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
    /// Terminal image protocol for the current framebuffer; recreated each
    /// emulated frame so a window resize re-fits without a keypress.
    image: Option<StatefulProtocol>,
    /// Last computed effective scale (for the status panel).
    scale: f32,
    // FPS accounting.
    frames: u32,
    fps: f32,
    fps_mark: Instant,
    /// Per-button last press time (index by [`key_index`]); drives auto-release.
    held: [Option<Instant>; 8],
    /// Host-audio stream for this game. Must stay alive for the whole session —
    /// dropping it stops the sound — so it is dropped when the session ends or a
    /// different ROM is loaded. `()` when built without the `audio` feature.
    _audio: AudioHold,
}

struct App {
    config: Config,
    recent: Recent,
    picker: Picker,
    palette: Palette,
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
}

impl App {
    fn new(config: Config, recent: Recent) -> App {
        // Prefer the sharpest protocol the terminal advertises (build_picker),
        // then let an explicit config `protocol` override it.
        let mut picker = build_picker();
        picker.set_background_color([0, 0, 0, 0]);
        if let Some(pref) = config.protocol.as_deref().and_then(protocol_from_str) {
            picker.set_protocol_type(pref);
        }
        let palette = config.palette;

        let mut app = App {
            config,
            recent,
            picker,
            palette,
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
        };
        app.rescan();
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
        match load_session(path) {
            Ok((gb, entry, audio)) => {
                self.recent.record(path);
                self.recent.save();
                self.session = Some(Session {
                    gb,
                    entry,
                    paused: false,
                    image: None,
                    scale: 1.0,
                    frames: 0,
                    fps: 0.0,
                    fps_mark: Instant::now(),
                    held: [None; 8],
                    _audio: audio,
                });
                self.screen = Screen::Playing;
                self.overlay = Overlay::None;
                self.fullscreen = false;
                self.message = None;
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
        let mut last_tick = Instant::now();
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
                if running {
                    let level = &SPEED_LADDER[self.speed];
                    if level.uncapped {
                        for _ in 0..level.burst {
                            self.step_frame();
                        }
                        last_tick = Instant::now();
                        dirty = true;
                    } else if last_tick.elapsed() >= level.interval() {
                        self.step_frame();
                        last_tick = Instant::now();
                        dirty = true;
                    }
                }
                // 2. Auto-release keys that were not refreshed.
                if self.release_expired() {
                    dirty = true;
                }
            }

            // 3. Draw only when something changed (bounds encode cost).
            if dirty {
                terminal.draw(|f| self.render(f))?;
                dirty = false;
            }

            // 4. Poll input with a short timeout so input feels immediate.
            if event::poll(Duration::from_millis(4))? {
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
        self.recent.save();
        Ok(())
    }

    fn step_frame(&mut self) {
        // Split borrows: pull the fields we need out of `self` first.
        let palette = self.palette;
        let picker = self.picker;
        if let Some(s) = self.session.as_mut() {
            s.gb.frame();
            let img = framebuffer_image(&s.gb, palette);
            s.image = Some(picker.new_resize_protocol(img));
            s.frames += 1;
            let elapsed = s.fps_mark.elapsed();
            if elapsed >= Duration::from_secs(1) {
                s.fps = s.frames as f32 / elapsed.as_secs_f32();
                s.frames = 0;
                s.fps_mark = Instant::now();
            }
        }
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
        let palette = self.palette;
        let picker = self.picker;
        let outcome = if let Some(s) = self.session.as_mut() {
            let path = state_path(&s.gb.rom_title(), slot);
            match std::fs::read(&path) {
                Ok(bytes) => match s.gb.load_state(&bytes) {
                    Ok(()) => {
                        // Refresh the on-screen frame without advancing emulation.
                        let img = framebuffer_image(&s.gb, palette);
                        s.image = Some(picker.new_resize_protocol(img));
                        Ok(())
                    }
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

        if self.fullscreen {
            self.render_display(f, area);
            self.render_stats_overlay(f, area);
            self.render_toast(f, area);
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

        self.render_display(f, body[0]);
        self.render_sidebar(f, body[1]);
        self.render_stats_overlay(f, body[0]);
        self.render_toast(f, body[0]);

        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " ?help · p pause · +/- speed · t stats · F5/F8 save/load · f full · q quit",
                Style::default().fg(Color::DarkGray),
            ))),
            rows[2],
        );
    }

    /// Small stats overlay pinned to the top-left of the game picture. Off by
    /// default (`t`); reads already-computed values so it costs no emulation
    /// time. Shows FPS, emulated speed %, frame time, PC and ROM bank.
    fn render_stats_overlay(&self, f: &mut Frame<'_>, area: Rect) {
        if !self.stats {
            return;
        }
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let speed_pct = s.fps as f64 / 59.7275 * 100.0;
        let frame_ms = if s.fps > 0.0 {
            1000.0 / s.fps as f64
        } else {
            0.0
        };
        let lines = vec![
            Line::from(format!("fps {:.0}", s.fps)),
            Line::from(format!("spd {speed_pct:.0}%")),
            Line::from(format!("ft  {frame_ms:.1}ms")),
            Line::from(format!("pc  {:04X}", s.gb.debug_pc())),
            Line::from(format!("bnk {}", s.gb.debug_rom_bank())),
        ];
        let w = 12u16.min(area.width);
        let h = (lines.len() as u16 + 2).min(area.height);
        let rect = Rect::new(area.x, area.y, w, h);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" stats "))
                .style(Style::default().fg(Color::Green)),
            rect,
        );
    }

    /// Transient save/load/speed notice, bottom-left of the game picture.
    fn render_toast(&self, f: &mut Frame<'_>, area: Rect) {
        let Some(msg) = self.toast_text() else { return };
        let text = format!(" {msg} ");
        let w = (text.chars().count() as u16).min(area.width);
        if w == 0 || area.height == 0 {
            return;
        }
        let rect = Rect::new(area.x, area.y + area.height - 1, w, 1);
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
    fn render_display(&mut self, f: &mut Frame<'_>, area: Rect) {
        let (font_w, font_h) = self.picker.font_size();
        let (rect, scale) = fit_metrics(area, 160, 144, font_w, font_h);
        if let Some(s) = self.session.as_mut() {
            s.scale = scale;
            if let Some(proto) = s.image.as_mut() {
                let image = StatefulImage::default().resize(Resize::Scale(None));
                f.render_stateful_widget(image, rect, proto);
            }
        }
    }

    fn render_sidebar(&mut self, f: &mut Frame<'_>, area: Rect) {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(11), Constraint::Min(0)])
            .split(area);

        // Status.
        let (title, mbc, scale, fps, paused) = self
            .session
            .as_ref()
            .map(|s| (s.entry.title.clone(), s.entry.mbc, s.scale, s.fps, s.paused))
            .unwrap_or_default();
        let status = vec![
            kv("ROM", &truncate(&title, 18)),
            kv("MBC", mbc),
            kv("Scale", &format!("{scale:.1}x")),
            kv("Proto", protocol_name(self.picker.protocol_type())),
            kv("Palette", self.palette.name()),
            kv("Speed", SPEED_LADDER[self.speed].label),
            kv("Slot", &self.slot.to_string()),
            kv(
                "FPS",
                &if paused {
                    "paused".to_string()
                } else {
                    format!("{fps:.0}")
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
            Line::from("  ?         this help   q       quit"),
            Line::from(""),
            Line::from(Span::styled(
                "press any key to close",
                Style::default().fg(Color::DarkGray),
            )),
        ];
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

/// What keeps the game's host-audio alive. The concrete cpal stream when built
/// with `--features audio`, otherwise a zero-sized `()`.
#[cfg(feature = "audio")]
type AudioHold = Option<cpal::Stream>;
#[cfg(not(feature = "audio"))]
type AudioHold = ();

/// Wire host audio into `gb`, returning the stream to keep alive. Feature-gated
/// (cpal pulls in ALSA) and graceful when no output device is available. Mirror
/// of `Gameboy::render_desktop`'s audio init, but per-session so it follows the
/// active game across in-app ROM switches. Deliberately silent (no `eprintln`)
/// because we are inside the alternate screen — a stray line would corrupt it.
#[cfg(feature = "audio")]
fn init_audio(gb: &mut Gameboy) -> AudioHold {
    match crate::sound::cpal_audio::CpalPlayer::new() {
        Some((player, stream)) => {
            gb.enable_audio(Box::new(player));
            Some(stream)
        }
        None => None,
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

/// Convert the emulator's RGBA framebuffer into an RGB [`DynamicImage`],
/// applying the configured palette to greyscale (DMG) output.
fn framebuffer_image(gb: &Gameboy, palette: Palette) -> DynamicImage {
    let width = gb.width;
    let height = gb.height;
    let input = gb.image();
    let mut out = vec![0u8; width as usize * height as usize * 3];

    let mut i = 0;
    for chunk in input.chunks(4) {
        let (r, g, b) = (chunk[0], chunk[1], chunk[2]);
        // DMG output is greyscale (r==g==b); recolour those via the palette.
        // Colour (CGB) pixels pass through untouched.
        if r == g && g == b {
            let px = palette.map(r);
            out[i..i + 3].copy_from_slice(&px);
        } else {
            out[i] = r;
            out[i + 1] = g;
            out[i + 2] = b;
        }
        i += 3;
    }

    let buffer = image::ImageBuffer::from_raw(width, height, out)
        .expect("framebuffer size matches width*height*3");
    DynamicImage::ImageRgb8(buffer)
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
}

use crate::gameboy::Gameboy;
use crate::input::KeypadKey;
use std::env;
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};
use std::{
    error::Error,
    io,
    time::{Duration, Instant},
};

use ratatui::{
    backend::{Backend, CrosstermBackend},
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
            KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{
            disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
        },
    },
    Terminal,
};

use image::DynamicImage;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::{Protocol, StatefulProtocol},
    Resize, StatefulImage,
};

/// Width (in terminal cells) reserved for the controls sidebar; the rest of the
/// screen goes to the game picture.
const SIDEBAR_WIDTH: u16 = 28;

/// Build a [`Picker`], preferring the sharpest graphics protocol the terminal
/// actually advertises.
///
/// `Picker::from_query_stdio()` lets an env-var guess (e.g. `TERM_PROGRAM=WezTerm`)
/// force the fuzzy iTerm2 protocol even when the terminal advertises the much
/// crisper Kitty graphics protocol. We first query capabilities with those env
/// guesses suppressed, so the *advertised* protocol wins; only if the terminal
/// advertises no real graphics protocol do we fall back to the env-based guess.
/// Never panics on a query timeout — falls back to a sane default instead.
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
        // The terminal advertised a real graphics protocol (Kitty/Sixel/iTerm2) — use it.
        Ok(p) if p.protocol_type() != ProtocolType::Halfblocks => p,
        // Otherwise fall back to the env-guess query (e.g. a genuine iTerm2), and if even
        // that fails (timeout / no capabilities) use a default rather than panicking.
        _ => {
            Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((10, 20)))
        }
    }
}

/// Largest integer source-scale whose upscaled 160×144 frame still covers the
/// on-screen game area, so `Resize::Fit` downsamples (crisp) instead of upsampling
/// a tiny source (blurry). Clamped to keep per-frame resizing cheap.
fn fit_scale(picker: &Picker, game_cols: u16, rows: u16) -> u32 {
    let (font_w, font_h) = picker.font_size();
    let area_w = game_cols as u32 * font_w.max(1) as u32;
    let area_h = rows as u32 * font_h.max(1) as u32;
    let by_w = area_w.div_ceil(160);
    let by_h = area_h.div_ceil(144);
    by_w.max(by_h).clamp(1, 5)
}

pub fn run(gameboy: Gameboy) -> Result<(), Box<dyn Error>> {
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

    // Initialize audio for the terminal frontend too (the desktop path does the same in
    // render_desktop). Requires the `audio` feature; degrades gracefully with no device.
    // `_audio_stream` must live for the whole run — dropping it stops the sound.
    #[cfg(all(not(target_arch = "wasm32"), feature = "audio"))]
    let mut gameboy = gameboy;
    #[cfg(all(not(target_arch = "wasm32"), feature = "audio"))]
    let _audio_stream = match crate::sound::cpal_audio::CpalPlayer::new() {
        Some((player, stream)) => {
            gameboy.enable_audio(Box::new(player));
            Some(stream)
        }
        None => {
            eprintln!("audio: no output device available; continuing without sound");
            None
        }
    };
    #[cfg(any(target_arch = "wasm32", not(feature = "audio")))]
    let _audio_stream: Option<()> = None;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        disable_raw_mode().unwrap();
        ratatui::crossterm::execute!(io::stdout(), LeaveAlternateScreen).unwrap();
        original_hook(panic);
    }));

    // setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::all())
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(&mut terminal, &gameboy);

    // run app
    let res = run_app(&mut terminal, app, Arc::new(Mutex::new(gameboy)));

    // restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        PopKeyboardEnhancementFlags
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: App,
    gameboy: Arc<Mutex<Gameboy>>,
) -> io::Result<()> {
    let mut last_tick = Instant::now();
    let cloned_gameboy = gameboy.clone();

    // Open at the largest scale that fits the current terminal, not a fixed 1x, so the
    // picture is usefully sized from the start. `o` cycles 1..=max_scale.
    let (cols, rows) = ratatui::crossterm::terminal::size().unwrap_or((80, 24));
    let game_cols = cols.saturating_sub(SIDEBAR_WIDTH);
    let max_scale = fit_scale(&app.picker, game_cols, rows);
    let scale = Arc::new(AtomicU32::new(max_scale));
    let scale_me = scale.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_me = stop.clone();
    let change_protocol = Arc::new(AtomicBool::new(false));
    let change_protocol_me = change_protocol.clone();

    std::thread::spawn(move || {
        loop {
            // let timeout = app
            //     .tick_rate
            //     .checked_sub(last_tick.elapsed())
            //     .unwrap_or_else(|| Duration::from_secs(0));
            let timeout = Duration::from_millis(5);
            if ratatui::crossterm::event::poll(timeout).is_ok() {
                if let Ok(Event::Key(key)) = event::read() {
                    let is_pressed = key.kind == KeyEventKind::Press;

                    if let Ok(mut gameboy) = cloned_gameboy.lock() {
                        if let KeyCode::Char(c) = key.code {
                            match (c, is_pressed) {
                                ('q', true) => {
                                    stop.store(true, Ordering::Relaxed);
                                }
                                ('i', true) => {
                                    change_protocol.store(true, Ordering::Relaxed);
                                }
                                ('o', true) => {
                                    let s = scale.load(Ordering::Relaxed);
                                    if s >= max_scale {
                                        scale.store(1, Ordering::Relaxed);
                                    } else {
                                        scale.store(s + 1, Ordering::Relaxed);
                                    }
                                }
                                // ('H', true) => {
                                //     if self.split_percent >= 10 {
                                //         self.split_percent -= 10;
                                //     }
                                // }
                                // ('L', true) => {
                                //     if self.split_percent <= 90 {
                                //         self.split_percent += 10;
                                //     }
                                // }
                                ('a', true) | ('A', true) => {
                                    gameboy.keydown(KeypadKey::A);
                                }
                                ('b', true) | ('B', true) => {
                                    gameboy.keydown(KeypadKey::B);
                                }
                                ('z', true) | ('Z', true) => {
                                    gameboy.keydown(KeypadKey::Select);
                                }
                                ('x', true) | ('X', true) => {
                                    gameboy.keydown(KeypadKey::Start);
                                }
                                ('a', false) | ('A', false) => {
                                    gameboy.keyup(KeypadKey::A);
                                }
                                ('b', false) | ('B', false) => {
                                    gameboy.keyup(KeypadKey::B);
                                }
                                ('z', false) | ('Z', false) => {
                                    gameboy.keyup(KeypadKey::Select);
                                }
                                ('x', false) | ('X', false) => {
                                    gameboy.keyup(KeypadKey::Start);
                                }
                                _ => {}
                            }
                        } else if let KeyCode::Up = key.code {
                            if is_pressed {
                                gameboy.keydown(KeypadKey::Up);
                            } else {
                                gameboy.keyup(KeypadKey::Up);
                            }
                        } else if let KeyCode::Down = key.code {
                            if is_pressed {
                                gameboy.keydown(KeypadKey::Down);
                            } else {
                                gameboy.keyup(KeypadKey::Down);
                            }
                        } else if let KeyCode::Left = key.code {
                            if is_pressed {
                                gameboy.keydown(KeypadKey::Left);
                            } else {
                                gameboy.keyup(KeypadKey::Left);
                            }
                        } else if let KeyCode::Right = key.code {
                            if is_pressed {
                                gameboy.keydown(KeypadKey::Right);
                            } else {
                                gameboy.keyup(KeypadKey::Right);
                            }
                        }
                    }
                }
            }
        }
    });

    loop {
        if change_protocol_me.load(Ordering::Relaxed) {
            app.change_image_protocol();
            change_protocol_me.store(false, Ordering::Relaxed);
        }

        terminal.draw(|f| ui(f, &mut app))?;

        if last_tick.elapsed() >= app.tick_rate {
            if let Ok(mut gameboy) = gameboy.lock() {
                gameboy.frame();
                app.on_tick(&mut gameboy, scale_me.load(Ordering::Relaxed));
            }
            last_tick = Instant::now();
        }
        if stop_me.load(Ordering::Relaxed) {
            return Ok(());
        }
    }
}

struct App {
    tick_rate: Duration,
    // split_percent: u16,
    picker: Picker,
    image_source: DynamicImage,
    image_static: Protocol,
    image_fit_state: StatefulProtocol,
}

fn size() -> Rect {
    Rect::new(0, 0, 30, 16)
}

#[inline]
fn get_image(gameboy: &Gameboy, scale: u32) -> image::DynamicImage {
    // let harvest_moon = "/Users/rapha/harvest-moon.png";
    // image::io::Reader::open(harvest_moon).unwrap().decode().unwrap()

    let width = gameboy.width;
    let height = gameboy.height;

    // Get the raw image data as a vector
    let input: &[u8] = gameboy.image();

    // Allocate a new buffer for the RGB image, 3 bytes per pixel
    let mut output_data = vec![0u8; width as usize * height as usize * 3];

    let mut i = 0;
    // Iterate through 4-byte chunks of the image data (RGBA bytes)
    for chunk in input.chunks(4) {
        // ... and copy each of them to output, leaving out the A byte
        output_data[i..i + 3].copy_from_slice(&chunk[0..3]);
        i += 3;
    }

    let mut buffer = image::ImageBuffer::from_raw(width, height, output_data).unwrap();
    if scale > 1 {
        buffer = image::imageops::resize(
            &buffer,
            width * scale,
            height * scale,
            image::imageops::FilterType::Nearest,
        );
    }
    image::DynamicImage::ImageRgb8(buffer)
}

impl App {
    pub fn new<B: Backend>(_: &mut Terminal<B>, gameboy: &Gameboy) -> Self {
        let image_source = get_image(gameboy, 1);

        let mut picker = build_picker();
        picker.set_background_color([0, 0, 0, 0]);

        let image_static = picker
            .new_protocol(image_source.clone(), size(), Resize::Fit(None))
            .unwrap();
        let image_fit_state = picker.new_resize_protocol(image_source.clone());

        Self {
            tick_rate: Duration::from_millis(5),
            // split_percent: 40,
            picker,
            image_source,

            image_static,
            image_fit_state,
        }
    }

    #[inline]
    fn reset_images(&mut self) {
        self.image_static = self
            .picker
            .new_protocol(self.image_source.clone(), size(), Resize::Fit(None))
            .unwrap();
        self.image_fit_state = self.picker.new_resize_protocol(self.image_source.clone());
    }

    #[inline]
    pub fn change_image_protocol(&mut self) {
        self.picker
            .set_protocol_type(self.picker.protocol_type().next());
        self.reset_images();
    }

    #[inline]
    pub fn on_tick(&mut self, gameboy: &mut Gameboy, scale: u32) {
        self.image_source = get_image(gameboy, scale);
        self.image_static = self
            .picker
            .new_protocol(self.image_source.clone(), size(), Resize::Fit(None))
            .unwrap();
        self.image_fit_state = self.picker.new_resize_protocol(self.image_source.clone());
    }

    #[inline]
    fn render_resized_image(&mut self, f: &mut Frame<'_>, resize: Resize, area: Rect) {
        let title = format!(
            "Gameboy on {} terminal",
            env::var("TERM").unwrap_or("unknown".to_string())
        );
        let (state, name, _color) = (&mut self.image_fit_state, title, Color::Black);
        let block = block(&name);
        let inner_area = block.inner(area);
        let image = StatefulImage::default().resize(resize);
        f.render_stateful_widget(image, inner_area, state);
        f.render_widget(block, area);
    }
}

#[inline]
fn ui(f: &mut Frame<'_>, app: &mut App) {
    let outer_block = Block::default();

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                // Give the game everything except a fixed-width controls sidebar, so the
                // picture is as large as the terminal allows instead of a cramped 40%.
                Constraint::Min(0),
                Constraint::Length(SIDEBAR_WIDTH),
            ]
            .as_ref(),
        )
        .split(outer_block.inner(f.area()));
    f.render_widget(outer_block, f.area());

    app.render_resized_image(f, Resize::Fit(None), chunks[0]);

    let block_right_bottom = block("Controls");
    let area = block_right_bottom.inner(chunks[1]);
    f.render_widget(
        paragraph(vec![
            Line::from("Controls:"),
            Line::from("arrows: movement"),
            Line::from("Key a/A: A"),
            Line::from("Key b/B: B"),
            Line::from("Key z/Z: select"),
            Line::from("Key x/X: start"),
            // Line::from("H/L: resize splits"),
            Line::from("o: scale image"),
            Line::from(format!(
                "i: cycle image protocols (current: {:?})",
                app.picker.protocol_type()
            )),
        ]),
        area,
    );
}

fn paragraph<'a, T: Into<Text<'a>>>(str: T) -> Paragraph<'a> {
    Paragraph::new(str).wrap(Wrap { trim: true })
}

fn block(name: &str) -> Block<'_> {
    Block::default().borders(Borders::ALL).title(name)
}

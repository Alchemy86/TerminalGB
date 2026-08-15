//! Watch a **scripted** run — the live picture, plus a caption saying what the
//! script is doing right now.
//!
//! The terminal frontend ([`crate::screen::tui`]) exists to let a *person* play
//! a game: it owns the keyboard, attaches plugins, wires a battery to the
//! cartridge and installs an audio player. None of that is wanted when a
//! headless script is driving the machine and the point is only to see it
//! happen — and two of them (the battery and the audio player) would change
//! what the script produces, which for something like
//! `shiny_hunt --make-state` is the whole deliverable.
//!
//! So this is not a second frontend and it is not a second renderer. It is the
//! frontend's **drawing** half with nothing else attached: the same protocol
//! negotiation (`tui::build_picker`), the same palette stage
//! ([`crate::screen::frame::FramePrep`]), the same upscaler, the same encoder
//! and cell placement ([`crate::termimage::TermImage`]), the same crisp
//! `▀` sampler ([`crate::screen::halfblocks`]), the same layout arithmetic
//! (`tui::fit_metrics`). The caller owns the `Gameboy` and decides when a frame
//! runs; this only ever *reads* the framebuffer. Those four `tui` helpers are
//! `pub(crate)` for this module and no other reason — do not make them private
//! again without moving them here.
//!
//! Three rules from `docs/terminal-render.md` apply here exactly as they do in
//! the frontend, and all three are obeyed below:
//!
//! * the image is transmitted at the **picture's** resolution, never the
//!   window's;
//! * the escape sequence is written straight to stdout **after** `ratatui` has
//!   flushed, never stuffed into a cell symbol;
//! * one image id for the whole run, never a fresh one per frame.
//!
//! ```no_run
//! # use terminalgb::gameboy::Gameboy;
//! # use terminalgb::screen::watch::Viewer;
//! # fn demo(gb: &mut Gameboy) -> std::io::Result<()> {
//! let mut view = Viewer::open("building a save state")?;
//! view.say("mashing through the intro");
//! for _ in 0..600 {
//!     gb.frame();
//!     view.pace_and_draw(gb.image())?;
//!     if view.aborted() {
//!         break;
//!     }
//! }
//! view.close();
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        execute,
        terminal::{
            disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
        },
    },
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use ratatui_image::{
    picker::ProtocolType, protocol::StatefulProtocol, Resize, StatefulImage,
};

use crate::screen::config::{Config, RenderPath};
use crate::screen::frame::FramePrep;
use crate::screen::tui::{build_picker, fit_metrics, probe_terminal, shm_supported};
use crate::termimage::{self, CellRect, TermImage, Wire};
use crate::upscale::Upscaler;

/// The DMG's true frame period — 70224 cycles at 4.194304 MHz. The same
/// constant the frontend paces on; a rounded 16 ms is 4.64 % fast.
pub const FRAME_TIME: Duration = Duration::from_micros(16_743);

/// A distinct Kitty image id. The frontend's game picture is `0x00676201` and
/// its panel pictures take the three ids above that, so this sits clear of all
/// four — a shared id would have two pictures overwrite each other.
const IMAGE_ID: u32 = 0x0067_6210;

/// How many finished captions stay on screen under the live one.
const HISTORY: usize = 3;

/// How often the picture is redrawn, at most. Emulation may run far faster;
/// **display frames are dropped, emulated frames never are** — the same rule
/// the frontend follows, and here it is what keeps `--show-speed max` honest.
const MIN_DRAW_GAP: Duration = FRAME_TIME;

/// A live picture with a caption, drawn over a terminal the caller has not
/// otherwise claimed.
pub struct Viewer {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    picker: ratatui_image::picker::Picker,
    prep: FramePrep,
    upscaler: Upscaler,
    tx_factor: u8,
    palette: crate::screen::config::Palette,
    /// Fast path: our own encoder, for a protocol with cell-addressed
    /// placement (Kitty, iTerm2). `None` for sixel and half-blocks.
    term_image: Option<TermImage>,
    /// `ratatui-image` protocol state, used for sixel.
    image: Option<StatefulProtocol>,
    crisp_halfblocks: bool,
    legacy_hash: u64,
    /// The escape sequence to write once `ratatui` has flushed.
    pending_wire: String,
    title: String,
    caption: String,
    detail: String,
    history: VecDeque<String>,
    /// Layout of the last draw, so a resize forces a retransmission.
    last_layout: Option<(Rect, Rect)>,
    /// Wall-clock deadline for the next emulated frame. Advanced by exact
    /// periods, **never reset to `now`** — resetting it silently adds each
    /// iteration's overhead to the period, which is the bug that made the
    /// frontend run slow.
    next_frame: Instant,
    frame_period: Option<Duration>,
    last_draw: Instant,
    frames: u64,
    aborted: bool,
    restored: bool,
}

impl Viewer {
    /// Claim the terminal and negotiate a graphics protocol.
    ///
    /// Honours the user's `config.toml` for palette, upscaler, protocol
    /// override and HiDPI cell scale, so what they watch here looks like their
    /// own emulator rather than a second set of defaults.
    pub fn open(title: &str) -> io::Result<Viewer> {
        // Refuse before touching anything, and say what is wrong. Left to
        // crossterm this is `No such device or address (os error 6)` from a
        // failed `/dev/tty` open, which is true and tells nobody that they
        // piped the output of a program whose whole purpose is to be looked
        // at. The frontend's rule applies: detect, adapt, *inform*.
        if !io::stdout().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "there is no terminal to draw on — stdout is redirected. Run it in \
                 a terminal, or drop the flag for the headless run",
            ));
        }
        let config = Config::load();

        // Restore the terminal if the caller panics. A crash must not leave
        // somebody in raw mode on an alternate screen.
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            original(panic);
        }));

        enable_raw_mode()?;
        // Ask the terminal what it can do while raw mode is on but nothing is
        // competing for stdin — the same window the frontend uses, and the
        // only one in which the replies are readable.
        probe_terminal();
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;

        let picker = build_picker();
        let wire = match picker.protocol_type() {
            _ if config.render_path == RenderPath::Legacy => None,
            ProtocolType::Kitty => Some(Wire::Kitty),
            ProtocolType::Iterm2 => Some(Wire::Iterm2),
            _ => None,
        };
        let term_image = wire.map(|w| {
            let mut img = TermImage::new(w, IMAGE_ID);
            if w == Wire::Kitty && shm_supported() {
                img.set_kitty_shm(true);
            }
            img
        });
        let crisp_halfblocks = config.render_path != RenderPath::Legacy
            && picker.protocol_type() == ProtocolType::Halfblocks;

        let upscaler = Upscaler::new(config.upscale, config.upscale_factor);
        let tx_factor = config
            .transmit_scale
            .factor_for(upscaler.algorithm(), upscaler.factor());

        Ok(Viewer {
            terminal,
            picker,
            prep: FramePrep::new(),
            upscaler,
            tx_factor,
            palette: config.palette,
            term_image,
            image: None,
            crisp_halfblocks,
            legacy_hash: 0,
            pending_wire: String::new(),
            title: title.to_string(),
            caption: String::new(),
            detail: String::new(),
            history: VecDeque::new(),
            last_layout: None,
            next_frame: Instant::now(),
            frame_period: Some(FRAME_TIME),
            last_draw: Instant::now() - MIN_DRAW_GAP,
            frames: 0,
            aborted: false,
            restored: false,
        })
    }

    /// Emulated frames per real second, as a multiple of the DMG's own
    /// 59.7275 Hz. `None` runs flat out.
    pub fn set_speed(&mut self, multiple: Option<f64>) {
        self.frame_period = match multiple {
            Some(m) if m > 0.0 => Some(FRAME_TIME.div_f64(m)),
            Some(_) => Some(FRAME_TIME),
            None => None,
        };
        self.next_frame = Instant::now();
    }

    /// Announce what the script is doing now. The previous caption drops into
    /// the dimmed history under it, so the run reads as a list of steps rather
    /// than a single flickering line.
    pub fn say(&mut self, caption: impl Into<String>) {
        let caption = caption.into();
        if caption == self.caption {
            return;
        }
        if !self.caption.is_empty() {
            self.history.push_back(std::mem::take(&mut self.caption));
            while self.history.len() > HISTORY {
                self.history.pop_front();
            }
        }
        self.caption = caption;
        self.detail.clear();
    }

    /// A second, dimmer line under the caption — where the player is, what
    /// frame it is on. Changes constantly; costs nothing when it does not.
    pub fn note(&mut self, detail: impl Into<String>) {
        self.detail = detail.into();
    }

    /// True once the watcher has pressed `q`, `Esc` or `Ctrl-C`.
    pub fn aborted(&self) -> bool {
        self.aborted
    }

    /// Emulated frames drawn through this viewer.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Wait until this emulated frame is due, redraw if a display frame is due
    /// too, and check for an abort. Call once per `Gameboy::frame`.
    pub fn pace_and_draw(&mut self, rgba: &[u8]) -> io::Result<()> {
        self.frames += 1;
        if let Some(period) = self.frame_period {
            let now = Instant::now();
            if now < self.next_frame {
                std::thread::sleep(self.next_frame - now);
            }
            // Advance by exact periods. If we have fallen more than a second
            // behind — a stalled terminal, a suspended process — resync rather
            // than replay the backlog at full tilt.
            self.next_frame += period;
            let now = Instant::now();
            if self.next_frame + Duration::from_secs(1) < now {
                self.next_frame = now;
            }
        }
        self.poll_input();
        if self.last_draw.elapsed() >= MIN_DRAW_GAP {
            self.draw(rgba)?;
        }
        Ok(())
    }

    /// Draw now, whatever the display schedule says. For a moment worth
    /// landing on: the caption has just changed, or the run is over.
    pub fn draw(&mut self, rgba: &[u8]) -> io::Result<()> {
        self.last_draw = Instant::now();

        // Everything the closure needs, out of `self` first: `terminal.draw`
        // borrows `self.terminal` mutably.
        let (font_w, font_h) = self.picker.font_size();
        let mapped = self.prep.map(rgba, self.palette);
        if self.upscaler.factor() != self.tx_factor {
            self.upscaler.set_factor(self.tx_factor);
        }
        let (tw, th) = self.upscaler.output_size(160, 144);
        let scaled = self.upscaler.apply(mapped, 160, 144);

        let title = &self.title;
        let caption = &self.caption;
        let detail = &self.detail;
        let history = &self.history;
        let term_image = &mut self.term_image;
        let image = &mut self.image;
        let legacy_hash = &mut self.legacy_hash;
        let pending = &mut self.pending_wire;
        let last_layout = &mut self.last_layout;
        let picker = &self.picker;
        let crisp = self.crisp_halfblocks;

        self.terminal.draw(|f| {
            let area = f.area();
            // Caption box: two rows of border, the live caption, the detail
            // line under it, and as much history as there is room for. Sized
            // from its own contents rather than a constant — a hard-coded
            // height beside a `push` is how a row that is being written ends
            // up clipped, which is a mistake this repository has made twice in
            // the frontend's own status panel.
            let want = 3 + history.len() as u16 + u16::from(!detail.is_empty());
            let foot = want.min((area.height / 3).max(3)).min(area.height);
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(foot),
                ])
                .split(area);

            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        " TerminalGB ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" {title}")),
                    Span::styled(
                        "   q to abort",
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ])),
                rows[0],
            );

            // Nothing may be drawn over a terminal image: the picture gets its
            // own rectangle and the caption gets its own. See
            // `docs/terminal-render.md`.
            let mut pic = rows[1];
            if term_image.as_ref().map(TermImage::wire) == Some(Wire::Kitty) {
                // Kitty addresses placeholder rows/columns from a fixed table,
                // so a placement cannot exceed `MAX_CELLS` on either axis.
                let max = termimage::MAX_CELLS;
                let w = pic.width.min(max);
                let h = pic.height.min(max);
                pic = Rect::new(
                    pic.x + (pic.width - w) / 2,
                    pic.y + (pic.height - h) / 2,
                    w,
                    h,
                );
            }
            let (rect, _scale) = fit_metrics(pic, 160, 144, font_w, font_h);

            let mut lines: Vec<Line> = history
                .iter()
                .map(|h| {
                    Line::from(Span::styled(
                        format!("  {h}"),
                        Style::default().add_modifier(Modifier::DIM),
                    ))
                })
                .collect();
            lines.push(Line::from(vec![
                Span::styled("▸ ", Style::default().fg(Color::Green)),
                Span::styled(
                    caption.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]));
            if !detail.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {detail}"),
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" what it is doing "),
                ),
                rows[2],
            );

            if rect.width == 0 || rect.height == 0 {
                return;
            }
            // A layout change may have painted over the terminal's copy of the
            // image, so the next transmission must be unconditional.
            let moved = *last_layout != Some((area, rect));
            *last_layout = Some((area, rect));

            match term_image.as_mut() {
                Some(term) => {
                    if moved {
                        term.invalidate();
                    }
                    let placement = term.place(
                        scaled,
                        tw as u32,
                        th as u32,
                        CellRect::new(rect.x, rect.y, rect.width, rect.height),
                    );
                    pending.clear();
                    if placement.changed {
                        pending.push_str(placement.seq);
                    }
                    // Mark the picture's cells `skip` so `ratatui` leaves them
                    // alone — but only on a frame that is *not* retransmitting,
                    // because a `skip` cell is never erased and stale text
                    // under a moved picture would show straight through.
                    if !placement.changed {
                        let buf = f.buffer_mut();
                        for y in 0..rect.height {
                            for x in 0..rect.width {
                                if let Some(cell) = buf.cell_mut((rect.x + x, rect.y + y))
                                {
                                    cell.set_skip(true);
                                }
                            }
                        }
                    }
                }
                None if crisp => {
                    // Two flat colours per cell, sampled nearest-neighbour.
                    // `ratatui-image`'s own half-block encoder hard-codes a
                    // smoothing filter; ours does not. See `screen::halfblocks`.
                    crate::screen::halfblocks::render(
                        scaled,
                        tw as u32,
                        th as u32,
                        rect,
                        f.buffer_mut(),
                    );
                }
                None => {
                    let hash = termimage::fnv1a(scaled);
                    if hash != *legacy_hash || image.is_none() || moved {
                        if let Some(buffer) = image::ImageBuffer::from_raw(
                            tw as u32,
                            th as u32,
                            scaled.to_vec(),
                        ) {
                            *image = Some(picker.new_resize_protocol(
                                image::DynamicImage::ImageRgba8(buffer),
                            ));
                        }
                        *legacy_hash = hash;
                    }
                    if let Some(proto) = image.as_mut() {
                        f.render_stateful_widget(
                            StatefulImage::default().resize(Resize::Scale(None)),
                            rect,
                            proto,
                        );
                    }
                }
            }
        })?;

        // After the flush, never inside a cell symbol: a 92 KB symbol makes
        // `ratatui`'s per-cell diff re-emit the whole screen every frame.
        if !self.pending_wire.is_empty() {
            let mut out = io::stdout();
            out.write_all(self.pending_wire.as_bytes())?;
            out.flush()?;
            self.pending_wire.clear();
        }
        Ok(())
    }

    /// Hold the finished picture up for `d`, still answering `q`.
    pub fn linger(&mut self, d: Duration) {
        let until = Instant::now() + d;
        while Instant::now() < until && !self.aborted {
            self.poll_input();
            std::thread::sleep(Duration::from_millis(20).min(FRAME_TIME));
        }
    }

    /// Give the terminal back. Idempotent, and also run by `Drop` — but call it
    /// explicitly before printing anything, or the summary lands on the
    /// alternate screen and vanishes with it.
    pub fn close(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        // A Kitty placement lives until it is deleted; leaving the alternate
        // screen without erasing can leave the picture behind.
        if let Some(term) = self.term_image.as_mut() {
            let mut out = io::stdout();
            let _ = out.write_all(term.erase().as_bytes());
            let _ = out.flush();
        }
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }

    fn poll_input(&mut self) {
        while event::poll(Duration::ZERO).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                        self.aborted = true
                    }
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.aborted = true
                    }
                    _ => {}
                },
                Ok(Event::Resize(_, _)) => self.last_layout = None,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.close();
    }
}

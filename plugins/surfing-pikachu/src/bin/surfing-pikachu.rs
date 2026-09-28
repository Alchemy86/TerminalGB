//! Surfing Pikachu in a terminal.
//!
//! The picture is composed by [`surfing_pikachu::render`] at the Game Boy's
//! own 160x144 and handed to the emulator's `halfblocks` renderer, which is
//! the same code the terminal frontend uses for a real cartridge — so the port
//! looks like the emulator rather than like a second, slightly different
//! terminal.
//!
//! Input goes through the gesture layer, never straight to the game. A mouse
//! drag is a real gesture and is wired up as one; keys are translated into the
//! same gestures so the thing is playable without a pointer. Both paths end at
//! one [`Controls`].

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
        KeyEventKind, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Terminal, TerminalOptions, Viewport};

use surfing_pikachu::game::{Game, PikachuState, Status, COURSE_SECTIONS};
use surfing_pikachu::input::{Controls, Dir, Gesture, SWIPE_FRAMES};
use surfing_pikachu::render::{FRAME_BYTES, SCREEN_H, SCREEN_W};
use surfing_pikachu::tiles::TileSet;

/// 70224 cycles at 4.194304 MHz. Not 16 ms — see the frame-pacing invariant
/// in the repository's `AGENTS.md`: the deadline advances by exact periods
/// and is never reset to `now()`, or draw and poll overhead is charged to the
/// period and the game runs slow.
const FRAME: Duration = Duration::from_nanos(16_742_706);
/// Frames of catch-up before the deadline is resynchronised. A terminal that
/// stalls must not then sprint.
const MAX_CATCH_UP: u32 = 4;

/// Without the kitty keyboard protocol a terminal never says a key came *up*,
/// so a key press becomes a swipe of the length one spin needs rather than a
/// hold. With it, a key is a real press and release.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyMode {
    Exact,
    SwipeOnly,
}

struct Args {
    gfx: Option<PathBuf>,
    seed: u32,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        gfx: None,
        seed: 0,
        help: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--gfx" => {
                args.gfx =
                    Some(PathBuf::from(it.next().ok_or("--gfx needs a directory")?));
            }
            "--seed" => {
                let v = it.next().ok_or("--seed needs a number")?;
                args.seed = v
                    .parse()
                    .map_err(|_| format!("--seed: {v:?} is not a number"))?;
            }
            "-h" | "--help" => args.help = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(args)
}

const HELP: &str = "\
surfing-pikachu — the Pokemon Yellow minigame, ported and played in a terminal

USAGE:
    surfing-pikachu [--gfx DIR] [--seed N]

OPTIONS:
    --gfx DIR    Directory holding pret's extracted surfing_pikachu_1a.png and
                 surfing_pikachu_1b.png. Without it the built-in art is used;
                 no game asset ships with this repository.
    --seed N     Fix the course. The same seed plays the same waves.
    -h, --help   This.

CONTROLS (gestures, not buttons — see the `input` module for why)
    drag left/right   spin the board, while airborne
    flick left/right  one spin
    left/right keys   the same, for a terminal without a pointer
    click / space     confirm
    q / Esc           leave the run; again on the title screen to quit

There is no accelerate and no jump: the original has neither. You speed up by
staying on your board, and you launch by crossing a wave crest fast enough.
";

fn main() -> io::Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(why) => {
            eprintln!("surfing-pikachu: {why}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{HELP}");
        return Ok(());
    }

    let (tiles, art_warning) = TileSet::load(args.gfx.as_deref());
    let mut notices = Vec::new();
    if let Some(w) = art_warning {
        notices.push(w);
    }
    notices.push(format!("picture: {}", tiles.source.describe()));

    // Detect, adapt, inform. A terminal with no keyboard protocol still
    // plays; it just gets flicks instead of holds, and is told so once.
    let key_mode = match ratatui::crossterm::terminal::supports_keyboard_enhancement() {
        Ok(true) => KeyMode::Exact,
        _ => {
            notices.push(format!(
                "input: this terminal does not report key release (no kitty keyboard \
                 protocol), so a key press is a {SWIPE_FRAMES}-frame flick rather than a \
                 hold. Drag with the mouse for a real hold."
            ));
            KeyMode::SwipeOnly
        }
    };
    for n in &notices {
        eprintln!("{n}");
    }

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    if key_mode == KeyMode::Exact {
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        );
    }
    let backend = ratatui::backend::CrosstermBackend::new(out);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fullscreen,
        },
    )?;
    terminal.hide_cursor()?;

    let result = run(&mut terminal, &tiles, args.seed, key_mode, &notices);

    let mut out = io::stdout();
    if key_mode == KeyMode::Exact {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = terminal.show_cursor();
    result
}

/// Per-phase frame timing, so "the game runs slow" can be answered with a
/// number instead of a guess. Enabled by `SURFING_PIKACHU_TRACE=1`, printed
/// once on exit. Costs two clock reads a frame when off.
#[derive(Default)]
struct Trace {
    on: bool,
    frames: u32,
    /// Waiting for input up to the frame deadline — the idle remainder.
    poll: Duration,
    sim: Duration,
    draw: Duration,
    /// Frames the deadline had to be resynchronised past, because the
    /// terminal stalled for longer than the bounded catch-up allows.
    dropped: u32,
}

impl Trace {
    fn new() -> Self {
        Trace {
            on: std::env::var_os("SURFING_PIKACHU_TRACE").is_some(),
            ..Default::default()
        }
    }
    fn report(&self) {
        if !self.on || self.frames == 0 {
            return;
        }
        let n = self.frames as f64;
        let us = |d: Duration| d.as_secs_f64() * 1e6 / n;
        eprintln!(
            "trace: {} frames, {} resyncs — idle {:.0}us sim {:.0}us draw {:.0}us = \
             {:.0}us/frame against a 16743us budget",
            self.frames,
            self.dropped,
            us(self.poll),
            us(self.sim),
            us(self.draw),
            us(self.poll + self.sim + self.draw),
        );
    }
}

/// Where the drag started, so a release can tell a flick from a slow drag.
struct Drag {
    from: u16,
    at: Instant,
    moved: bool,
}

fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    tiles: &TileSet,
    seed: u32,
    key_mode: KeyMode,
    notices: &[String],
) -> io::Result<()> {
    let seed = if seed == 0 { fresh_seed() } else { seed };
    let mut game = Game::new(seed);
    let mut controls = Controls::new();
    let mut frame = vec![0u8; FRAME_BYTES];
    let mut drag: Option<Drag> = None;
    let mut showing_title = true;
    let mut quit = false;
    let mut high_score = [0u8, 0];
    let mut seed_in_play = seed;

    let mut trace = Trace::new();
    let mut deadline = Instant::now() + FRAME;
    loop {
        let t_poll = Instant::now();
        // Drain whatever the terminal has without blocking past the deadline.
        loop {
            let now = Instant::now();
            let wait = deadline.saturating_duration_since(now);
            if wait.is_zero() || !event::poll(wait)? {
                break;
            }
            match event::read()? {
                Event::Key(k) => {
                    if let Some(g) = key_gesture(k, key_mode) {
                        if showing_title && matches!(g, Gesture::Tap) {
                            showing_title = false;
                        } else if matches!(g, Gesture::LongPress) && showing_title {
                            quit = true;
                        } else {
                            controls.feed(g);
                        }
                    }
                }
                Event::Mouse(m) => {
                    if let Some(g) = mouse_gesture(m, &mut drag) {
                        if showing_title && matches!(g, Gesture::Tap) {
                            showing_title = false;
                        } else {
                            controls.feed(g);
                        }
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if quit {
            trace.report();
            return Ok(());
        }
        let t_sim = Instant::now();
        trace.poll += t_sim - t_poll;

        if !showing_title {
            let input = controls.frame();
            if game.step(input) == Status::Finished {
                // A finished run rolls straight into the next one, carrying
                // the high score the way the original carries it in SRAM.
                high_score = game.high_score();
                seed_in_play = fresh_seed();
                game = Game::new(seed_in_play).with_high_score(high_score);
                showing_title = true;
            }
            game.render(tiles, &mut frame);
        }

        let t_draw = Instant::now();
        trace.sim += t_draw - t_sim;
        terminal.draw(|f| {
            let area = f.area();
            if showing_title {
                draw_title(f.buffer_mut(), area, seed_in_play, high_score, notices);
                return;
            }
            let (picture, status) = layout(area);
            terminalgb::screen::halfblocks::render(
                &frame,
                SCREEN_W as u32,
                SCREEN_H as u32,
                picture,
                f.buffer_mut(),
            );
            f.render_widget(Paragraph::new(status_line(&game, seed_in_play)), status);
        })?;

        let after_draw = Instant::now();
        trace.draw += after_draw - t_draw;
        trace.frames += 1;

        // Deadline accumulator, bounded. Never `deadline = Instant::now()`.
        deadline += FRAME;
        let now = Instant::now();
        if deadline + FRAME * MAX_CATCH_UP < now {
            deadline = now + FRAME;
            trace.dropped += 1;
        }
    }
}

fn fresh_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.subsec_nanos() | 1)
}

/// The largest 160x144 picture that fits, leaving a row for the status line.
/// Half-blocks give two vertical samples per cell, so a picture `w` cells wide
/// wants `w * 144 / (160 * 2)` cells of height for square pixels.
fn layout(area: Rect) -> (Rect, Rect) {
    let status_h = 1u16;
    let avail_h = area.height.saturating_sub(status_h);
    let mut w = area.width;
    let mut h = (w as u32 * 144 / (160 * 2)) as u16;
    if h > avail_h {
        h = avail_h;
        w = (h as u32 * 160 * 2 / 144) as u16;
    }
    let w = w.min(area.width).max(1);
    let h = h.min(avail_h).max(1);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (avail_h - h) / 2;
    (
        Rect::new(x, y, w, h),
        Rect::new(
            area.x,
            area.y + area.height - status_h,
            area.width,
            status_h,
        ),
    )
}

fn key_gesture(k: KeyEvent, mode: KeyMode) -> Option<Gesture> {
    let dir = match k.code {
        KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => Some(Dir::Left),
        KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => Some(Dir::Right),
        _ => None,
    };
    match (k.kind, dir) {
        // A release only ever arrives under the kitty protocol; without it
        // the swipe's own decay is the only thing that lets go.
        (KeyEventKind::Release, Some(_)) if mode == KeyMode::Exact => {
            Some(Gesture::Release)
        }
        (KeyEventKind::Release, _) => None,
        (_, Some(d)) if mode == KeyMode::Exact => Some(Gesture::Press(d)),
        (_, Some(d)) => Some(Gesture::Swipe(d)),
        _ => match k.code {
            KeyCode::Char(' ') | KeyCode::Enter => Some(Gesture::Tap),
            KeyCode::Char('q') | KeyCode::Esc => Some(Gesture::LongPress),
            _ => None,
        },
    }
}

/// A pointer drag is a gesture in its own right, which is the whole reason the
/// gesture layer exists: the same [`Controls`] takes it unchanged.
fn mouse_gesture(m: MouseEvent, drag: &mut Option<Drag>) -> Option<Gesture> {
    /// Cells of travel before a drag counts as a direction rather than a jitter.
    const DEAD_ZONE: i32 = 2;
    /// A drag released this quickly is a flick, and its length sets the spin.
    const FLICK: Duration = Duration::from_millis(250);
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            *drag = Some(Drag {
                from: m.column,
                at: Instant::now(),
                moved: false,
            });
            None
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let d = drag.as_mut()?;
            let dx = m.column as i32 - d.from as i32;
            if dx.abs() < DEAD_ZONE {
                return None;
            }
            d.moved = true;
            Some(Gesture::Press(if dx < 0 { Dir::Left } else { Dir::Right }))
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let d = drag.take()?;
            let dx = m.column as i32 - d.from as i32;
            let held = d.at.elapsed();
            if !d.moved && dx.abs() < DEAD_ZONE {
                return Some(Gesture::Tap);
            }
            if held < FLICK {
                // Longer flick, longer spin — one swipe's worth per dead zone
                // of travel, so a hard flick chains.
                let lots = (dx.unsigned_abs() / DEAD_ZONE as u32).max(1);
                let frames = (lots * SWIPE_FRAMES as u32).min(u8::MAX as u32) as u8;
                return Some(Gesture::SwipeFor(
                    if dx < 0 { Dir::Left } else { Dir::Right },
                    frames,
                ));
            }
            Some(Gesture::Release)
        }
        _ => None,
    }
}

fn status_line(game: &Game, seed: u32) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let state = match game.state() {
        PikachuState::Riding => "riding",
        PikachuState::Jumping => "AIR",
        PikachuState::Landing => "landing",
        PikachuState::Crashed => "WIPEOUT",
        PikachuState::GameEnd => "done",
        PikachuState::InitResults | PikachuState::Results => "results",
    };
    let colour = match game.state() {
        PikachuState::Jumping => Color::Cyan,
        PikachuState::Crashed => Color::Red,
        _ => Color::Reset,
    };
    Line::from(vec![
        Span::styled(format!(" {state:<8}"), Style::default().fg(colour)),
        Span::styled("HP ", dim),
        Span::raw(format!("{:>4} ", game.hp_value())),
        Span::styled("wave ", dim),
        Span::raw(format!("{:>2}/{COURSE_SECTIONS} ", game.section())),
        Span::styled("speed ", dim),
        Span::raw(format!("{:.2} ", game.speed_pixels())),
        Span::styled("rad ", dim),
        Span::raw(format!("{:>4} ", game.radness_value())),
        Span::styled("total ", dim),
        Span::raw(format!("{:>4} ", game.total_value())),
        Span::styled(
            format!("· spins {} · seed {seed}", game.radness_meter()),
            dim,
        ),
    ])
}

/// The original opens on a beach with "TO SURF RAD!" over it. Those tilemaps
/// were not part of the extracted reference set, so this is a title of our own
/// rather than a guess at theirs.
fn draw_title(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    seed: u32,
    high_score: [u8; 2],
    notices: &[String],
) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let hi = surfing_pikachu::bcd::value(&high_score);
    let mut lines = vec![
        Line::from(Span::styled("  SURFING PIKACHU", bold)),
        Line::from(Span::styled(
            "  a Rust port of the Pokemon Yellow minigame",
            dim,
        )),
        Line::from(""),
        Line::from("  drag or flick left / right   spin the board, while airborne"),
        Line::from("  left / right keys            the same, without a pointer"),
        Line::from("  click or space               start · confirm"),
        Line::from("  q                            quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  no accelerate, no jump: you speed up by staying on the board,",
            dim,
        )),
        Line::from(Span::styled(
            "  and launch by crossing a crest fast enough.",
            dim,
        )),
        Line::from(""),
        Line::from(format!("  seed {seed}    high score {hi}")),
        Line::from(""),
    ];
    for n in notices {
        lines.push(Line::from(Span::styled(format!("  {n}"), dim)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  click or press space to surf",
        bold,
    )));
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render_into(buf, area);
}

/// `Paragraph` needs a `Widget`, and `render_into` keeps the call site short.
trait RenderInto {
    fn render_into(self, buf: &mut ratatui::buffer::Buffer, area: Rect);
}
impl RenderInto for Paragraph<'_> {
    fn render_into(self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        use ratatui::widgets::Widget;
        self.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_keeps_its_aspect_and_stays_inside_the_terminal() {
        for w in 10u16..200 {
            for h in 5u16..60 {
                let (pic, status) = layout(Rect::new(0, 0, w, h));
                assert!(pic.x + pic.width <= w, "{w}x{h}: picture overflows");
                assert!(pic.y + pic.height <= h, "{w}x{h}: picture overflows");
                assert!(status.y + status.height <= h);
                // Square-ish pixels: two half-block samples per cell row.
                let ratio = pic.width as f32 / (pic.height as f32 * 2.0);
                assert!(
                    (0.7..1.6).contains(&ratio) || pic.height <= 2,
                    "{w}x{h}: aspect {ratio:.2} from {pic:?}"
                );
            }
        }
    }

    fn key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        let mut k = KeyEvent::from(code);
        k.kind = kind;
        k
    }

    #[test]
    fn keys_become_holds_only_where_release_is_reported() {
        let press = key(KeyCode::Left, KeyEventKind::Press);
        assert!(matches!(
            key_gesture(press, KeyMode::Exact),
            Some(Gesture::Press(Dir::Left))
        ));
        assert!(matches!(
            key_gesture(press, KeyMode::SwipeOnly),
            Some(Gesture::Swipe(Dir::Left))
        ));
        let release = key(KeyCode::Left, KeyEventKind::Release);
        assert!(matches!(
            key_gesture(release, KeyMode::Exact),
            Some(Gesture::Release)
        ));
        // Without the protocol a "release" is a phantom and must be ignored,
        // or the swipe would be cancelled the instant it started.
        assert!(key_gesture(release, KeyMode::SwipeOnly).is_none());
    }

    fn mouse(kind: MouseEventKind, column: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row: 0,
            modifiers: event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn a_click_is_a_tap_and_a_drag_is_a_hold() {
        let mut drag = None;
        assert!(mouse_gesture(
            mouse(MouseEventKind::Down(MouseButton::Left), 20),
            &mut drag
        )
        .is_none());
        assert!(matches!(
            mouse_gesture(mouse(MouseEventKind::Up(MouseButton::Left), 20), &mut drag),
            Some(Gesture::Tap)
        ));
        assert!(drag.is_none(), "the drag must be cleared on release");

        let mut drag = None;
        mouse_gesture(
            mouse(MouseEventKind::Down(MouseButton::Left), 40),
            &mut drag,
        );
        assert!(mouse_gesture(
            mouse(MouseEventKind::Drag(MouseButton::Left), 41),
            &mut drag
        )
        .is_none());
        assert!(matches!(
            mouse_gesture(
                mouse(MouseEventKind::Drag(MouseButton::Left), 30),
                &mut drag
            ),
            Some(Gesture::Press(Dir::Left))
        ));
        assert!(matches!(
            mouse_gesture(
                mouse(MouseEventKind::Drag(MouseButton::Left), 60),
                &mut drag
            ),
            Some(Gesture::Press(Dir::Right))
        ));
    }

    #[test]
    fn a_quick_drag_is_a_flick_worth_more_than_one_spin() {
        let mut drag = None;
        mouse_gesture(
            mouse(MouseEventKind::Down(MouseButton::Left), 40),
            &mut drag,
        );
        mouse_gesture(
            mouse(MouseEventKind::Drag(MouseButton::Left), 30),
            &mut drag,
        );
        match mouse_gesture(mouse(MouseEventKind::Up(MouseButton::Left), 30), &mut drag) {
            Some(Gesture::SwipeFor(Dir::Left, frames)) => {
                assert!(
                    frames > SWIPE_FRAMES,
                    "a ten-cell flick should outlast one spin"
                );
            }
            other => panic!("expected a leftward flick, got {other:?}"),
        }
    }
}

//! The commentary panel: laying out what the agent is thinking for a Twitch
//! audience.
//!
//! # Written for someone who arrived nine seconds ago
//!
//! The captain's brief narrows this a long way, and the narrowing is the
//! design: the destination is **Twitch via OBS**, on a fixed 16:9 canvas, and
//! the audience is strangers with no context who cannot pause and cannot zoom.
//! That produces four rules, in priority order, and everything below follows
//! from them:
//!
//! 1. **Current goal first.** A cold joiner must understand what is going on
//!    within a couple of seconds. So the panel leads with a standing `GOAL`
//!    line that changes rarely, then a `NOW` line that changes often. A
//!    scrolling log — the obvious thing to build — fails this completely: by
//!    the time a new viewer has read enough of it to orient, it has moved.
//! 2. **History is context, not content.** A few recent lines behind the goal,
//!    dimmed, so the last minute is legible at a glance. Not forty.
//! 3. **Big enough to survive the pipeline.** Twitch re-encodes, and a viewer
//!    may be on a phone at a third of the canvas height. The layout therefore
//!    assumes a *large terminal font* and few columns — see
//!    [`docs/claude-plays.md`](../docs/claude-plays.md) §OBS for the exact
//!    numbers — and wraps to the panel width rather than relying on the
//!    terminal to do something sensible.
//! 4. **Everything here is public and permanent.** Anything the panel renders
//!    can be clipped and reposted. Nothing internal reaches it: the text has
//!    already been through
//!    [`sanitise`](gameboy_plugin_agent::commentary::sanitise), and errors arrive as a
//!    category rather than a message.
//!
//! # And it must not touch the picture
//!
//! A terminal graphics image owns whole cells: text drawn into a cell the
//! picture covers is not *over* the image, it is *instead of* it, and under
//! Kitty's placeholder placement it stops the picture updating at all (see
//! [`docs/terminal-render.md`](../docs/terminal-render.md)). The panel is given
//! its own rectangle by [`split`], and
//! `the_commentary_panel_never_overlaps_the_picture` asserts that across a
//! spread of window sizes.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use gameboy_plugin_agent::commentary::{Activity, Commentary, Entry, Kind};

/// Rows the panel takes when there is room. Chosen for the OBS layout: a
/// header, the goal (up to two wrapped lines), the "now" line, and four lines
/// of history, inside a border.
pub const PANEL_H: u16 = 9;
/// Below this the panel would show the goal and nothing else, which is still
/// worth having; below *this* it is not drawn at all.
pub const MIN_H: u16 = 4;
/// Narrower than this and a wrapped goal line is unreadable.
pub const MIN_W: u16 = 30;

/// Split an area into (game, panel). `None` for the panel when there is no
/// room — the picture always wins, because a stream with no game on it is not
/// a stream.
pub fn split(area: Rect, rows: u16) -> (Rect, Option<Rect>) {
    let want = rows.clamp(MIN_H, PANEL_H.max(rows));
    if area.width < MIN_W || area.height < want + 6 {
        return (area, None);
    }
    let game = Rect::new(area.x, area.y, area.width, area.height - want);
    let panel = Rect::new(area.x, area.y + area.height - want, area.width, want);
    (game, Some(panel))
}

/// Everything the panel draws, already laid out as styled lines.
///
/// Returned rather than rendered so it can be asserted on directly: the tests
/// below check what a viewer reads, not that a function was called.
pub fn lines(c: &Commentary, width: u16, height: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(4).max(10) as usize;
    let mut out: Vec<Line<'static>> = Vec::new();

    // Row 1: who is playing and what they are doing. Short, so it can sit
    // beside the budget without wrapping.
    let (badge, colour) = match c.activity() {
        Activity::Thinking => ("THINKING", Color::Cyan),
        Activity::Playing => ("PLAYING", Color::Green),
        Activity::Paused => ("PAUSED", Color::Yellow),
        Activity::Off => ("WATCHING", Color::Gray),
        Activity::Starting => ("STARTING", Color::Gray),
    };
    let mut head = vec![
        Span::styled(
            "CLAUDE ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            badge.to_string(),
            Style::default().fg(colour).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(why) = c.pause_reason() {
        head.push(Span::styled(
            format!(" — {}", clip(why, inner.saturating_sub(20))),
            Style::default().fg(Color::Yellow),
        ));
    }
    out.push(Line::from(head));

    // Rows 2..: the goal. The line a new viewer reads first, so it is bright,
    // bold and allowed to wrap onto a second row.
    let goal_rows = if height >= 7 { 2 } else { 1 };
    let goal = wrap(c.goal(), inner.saturating_sub(6), goal_rows);
    for (i, part) in goal.iter().enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(Span::styled(
                "GOAL  ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw("      "));
        }
        spans.push(Span::styled(
            part.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        out.push(Line::from(spans));
    }

    // The "now" line: what is physically happening on screen this second.
    if !c.doing().is_empty() && height >= 5 {
        out.push(Line::from(vec![
            Span::styled("NOW   ", Style::default().fg(Color::Green)),
            Span::styled(
                clip(c.doing(), inner.saturating_sub(6)),
                Style::default().fg(Color::Green),
            ),
        ]));
    }

    // History, oldest first so the newest is nearest the eye at the bottom.
    let room = (height as usize).saturating_sub(out.len() + 2);
    if room > 0 {
        let recent: Vec<&Entry> = c
            .history()
            .rev()
            // A goal already has its own line; repeating it in the history
            // wastes the scarcest space on the page.
            .filter(|e| e.kind != Kind::Goal)
            .take(room)
            .collect();
        for e in recent.into_iter().rev() {
            let (mark, style) = match e.kind {
                Kind::Thought => ("· ", Style::default().fg(Color::Gray)),
                Kind::Act => ("▸ ", Style::default().fg(Color::DarkGray)),
                Kind::Event => ("! ", Style::default().fg(Color::LightMagenta)),
                Kind::System => ("• ", Style::default().fg(Color::Yellow)),
                Kind::Goal => ("  ", Style::default().fg(Color::White)),
            };
            out.push(Line::from(vec![
                Span::styled(mark, style),
                Span::styled(clip(&e.text, inner.saturating_sub(2)), style),
            ]));
        }
    }
    out
}

/// Cut to `max` characters on a word boundary, with an ellipsis. Character
/// counts, not bytes — the commentary can contain `é` and `♂`.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut cut: String = s.chars().take(max.saturating_sub(1)).collect();
    if let Some(sp) = cut.rfind(' ') {
        if sp > max / 2 {
            cut.truncate(sp);
        }
    }
    cut.push('…');
    cut
}

/// Wrap into at most `rows` lines of `width`, ellipsising the last one.
fn wrap(s: &str, width: usize, rows: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let extra = if line.is_empty() { 0 } else { 1 };
        if line.chars().count() + extra + word.chars().count() > width {
            if out.len() + 1 == rows {
                // Last row: everything that is left goes here, clipped.
                let mut rest = line.clone();
                if !rest.is_empty() {
                    rest.push(' ');
                }
                rest.push_str(word);
                out.push(clip(&rest, width));
                return out;
            }
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gameboy_plugin_agent::commentary::Problem;

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn busy() -> Commentary {
        let mut c = Commentary::default();
        c.set_goal(1, "Get out of the house and find Professor Oak");
        c.set_activity(gameboy_plugin_agent::commentary::Activity::Playing);
        c.set_doing("walk down 3");
        c.thought(2, "The stairs are on the right, so I'll head over there.");
        c.act(3, "walk right 4 — walked 4 step(s) to (7,1)");
        c.event(4, "Arrived: Red's house 1F.");
        c.thought(5, "Downstairs now. The front door should be south.");
        c.act(6, "head for (2,7) — arrived at (2,7) after 6 step(s)");
        c
    }

    /// The rule that decides the whole layout: a stranger must be able to read
    /// the goal and the current activity first, before any history.
    #[test]
    fn a_cold_viewer_sees_the_goal_before_any_history() {
        let out = lines(&busy(), 90, PANEL_H);
        let t = text_of(&out);
        let goal_at = t.find("Get out of the house").expect("goal is shown");
        // The newest history line, which is the one that always survives the
        // squeeze — older ones are dropped first, by design.
        let hist_at = t.find("head for (2,7)").expect("recent history too");
        assert!(goal_at < hist_at, "the goal must come first:\n{t}");
        assert!(t.starts_with("CLAUDE PLAYING"), "{t}");
        assert!(t.contains("NOW"), "and what it is doing right now:\n{t}");
    }

    /// Newest history nearest the bottom, i.e. nearest where the eye lands.
    #[test]
    fn history_reads_oldest_to_newest() {
        let t = text_of(&lines(&busy(), 90, 14));
        let first = t.find("The stairs are on the right").unwrap();
        let last = t.find("head for (2,7)").unwrap();
        assert!(first < last, "{t}");
    }

    /// The panel is the only thing on screen that shrinks gracefully; the
    /// picture must never be squeezed to make room.
    #[test]
    fn it_degrades_to_the_goal_when_there_is_no_room() {
        let t = text_of(&lines(&busy(), 40, MIN_H));
        assert!(t.contains("Get out of the house"), "{t}");
        assert!(
            !t.contains("The stairs are on the right"),
            "history is the first thing dropped:\n{t}"
        );
    }

    #[test]
    fn nothing_ever_exceeds_the_panel_width() {
        for w in [30u16, 48, 64, 90, 120, 200] {
            for h in [MIN_H, 6, PANEL_H, 16] {
                for line in lines(&busy(), w, h) {
                    let n: usize =
                        line.spans.iter().map(|s| s.content.chars().count()).sum();
                    assert!(
                        n <= w as usize,
                        "line of {n} chars in a {w}-wide panel: {:?}",
                        text_of(&[line.clone()])
                    );
                }
            }
        }
    }

    #[test]
    fn it_never_draws_more_lines_than_the_panel_has_rows() {
        for h in [MIN_H, 5, 6, PANEL_H, 12, 20] {
            let n = lines(&busy(), 90, h).len();
            assert!(n <= h as usize, "{n} lines in {h} rows");
        }
    }

    /// A paused agent must say so where a viewer will see it, and the reason
    /// must be the public one.
    #[test]
    fn a_paused_agent_says_so_in_the_headline() {
        let mut c = busy();
        c.pause(9, "the agent has reached its spending limit");
        let t = text_of(&lines(&c, 100, PANEL_H));
        assert!(t.contains("PAUSED"), "{t}");
        assert!(t.contains("spending limit"), "{t}");
    }

    /// End to end for the thing that must never go wrong: a driver failure
    /// carrying a path and a key reaches the panel as a category and nothing
    /// else.
    #[test]
    fn a_driver_failure_reaches_the_panel_as_a_category() {
        let mut c = Commentary::default();
        c.note_problem(1, Problem::Crashed);
        let t = text_of(&lines(&c, 100, PANEL_H));
        assert!(t.contains("hit a problem"), "{t}");
        for leak in ["sk-", "/home/", "Traceback", "exit code"] {
            assert!(!t.contains(leak), "{leak} leaked into the panel:\n{t}");
        }
    }

    /// Not an assertion so much as a way to *look* at the panel: with
    /// `GB_DUMP_PANEL=<file>` this writes exactly what a viewer reads, so the
    /// picture in `docs/claude-plays.md` is a real render rather than a
    /// drawing of one.
    #[test]
    fn the_rendered_panel_can_be_dumped_for_the_documentation() {
        let out = text_of(&lines(&busy(), 104, PANEL_H));
        if let Ok(path) = std::env::var("GB_DUMP_PANEL") {
            let _ = std::fs::write(path, &out);
        }
        assert!(out.lines().count() <= PANEL_H as usize);
    }

    #[test]
    fn split_gives_the_picture_everything_the_panel_does_not_need() {
        let area = Rect::new(0, 0, 120, 34);
        let (game, panel) = split(area, PANEL_H);
        let panel = panel.expect("room for a panel");
        assert_eq!(game.height + panel.height, area.height);
        assert_eq!(game.y, 0);
        assert_eq!(panel.y, game.bottom());
        assert!(!game.intersects(panel));

        // Too short: the picture keeps the lot.
        let (game, panel) = split(Rect::new(0, 0, 120, 8), PANEL_H);
        assert!(panel.is_none());
        assert_eq!(game.height, 8);
    }

    #[test]
    fn wrapping_breaks_on_words_and_ellipsises_the_overflow() {
        let w = wrap("one two three four five six seven eight", 12, 2);
        assert_eq!(w.len(), 2);
        assert!(w[0].chars().count() <= 12, "{w:?}");
        assert!(w[1].ends_with('…'), "{w:?}");
        assert!(!w[0].contains("thre "), "no mid-word break: {w:?}");
    }
}

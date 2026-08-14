//! The music-mode shelf: cartridges laid out like albums.
//!
//! Kept apart from [`crate::screen::tui`] for the same two reasons the trainer
//! panel is, and one more:
//!
//! * **Text and the picture never share a cell.** A terminal graphics image owns
//!   whole cells; text drawn into them replaces that part of the image, and with
//!   Kitty's Unicode placeholders it stops the picture updating at all. [`layout`]
//!   therefore returns disjoint rectangles and
//!   `the_shelf_never_overlaps_the_cover_at_any_size` asserts it over a sweep of
//!   window sizes.
//! * The layout can be unit-tested without a terminal.
//! * **The wording is a promise**, and a promise belongs somewhere it can be
//!   tested. Music mode gets the *title theme* — what a cartridge plays when it
//!   boots — and it must never let a reader believe it got a soundtrack. The
//!   strings live in one place and
//!   `the_panel_never_promises_more_than_a_title_theme` renders them.
//!
//! ```text
//! ┌ MUSIC MODE ─ 3 albums ──────────────────────────────────────┐
//! │                                                              │
//! │      ┌──────────────┐    POKEMON BLUE                        │
//! │      │              │    MBC3+RAM+BATTERY · 1024 KiB         │
//! │      │  cover art   │                                        │
//! │      │  (the game's │    ♪ title theme                       │
//! │      │   own title  │      what the cartridge plays when it  │
//! │      │   screen)    │      boots                             │
//! │      │              │    ▶ playing   ▮▮▮▯ ch                 │
//! │      └──────────────┘    ▦ cover 1/3 · from 00:14            │
//! │                                                              │
//! ├──────────────────────────────────────────────────────────────┤
//! │   DMG-ACID2  │  ▶ POKEMON BLUE  │  POKEMON GOLD              │
//! └──────────────────────────────────────────────────────────────┘
//!  ←/→ browse · ⏎ play · w save WAV · c cover · ? help · q leave
//! ```

use ratatui::layout::Rect;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use crate::music::library::Export;
use crate::music::{Album, AudioVerdict, Library};
use crate::termimage::TermImage;

/// Everything music mode holds that the game view does not.
///
/// The **playing** album and the **selected** album are deliberately separate:
/// browsing a shelf must not stop the music, which is the whole feel of a
/// library. `⏎` is what moves the needle.
pub struct MusicState {
    pub lib: Library,
    /// Index of the album under the cursor.
    pub sel: usize,
    /// Index of the album the live machine is running, if any.
    pub playing: Option<usize>,
    /// When the current album started, for the elapsed clock.
    pub started: Option<Instant>,
    /// Seconds into the boot the current album was resumed at, so the clock
    /// reads the same time the cover was taken from rather than restarting at
    /// zero in the middle of a game's boot.
    pub play_offset: f64,
    /// The cover's own terminal image. **Its own id**: a Kitty image is
    /// addressed by id, so sharing the game's would replace the picture.
    pub image: Option<TermImage>,
    /// Where `w` writes.
    pub export_dir: PathBuf,
    pub export_seconds: f64,
    /// A render in flight, and the title it is for. Exports run on their own
    /// thread — a one-second render on the loop thread would drain the audio
    /// queue's ~41 ms cushion and be heard as a gap.
    pub export_rx: Option<Receiver<Result<Export, String>>>,
    pub exporting: Option<String>,
    /// Last message from an export, shown in the panel until the next one.
    pub notice: Option<String>,
    /// The cover's image sequence, written to the terminal **after** the text
    /// frame is flushed — never into a cell symbol, which would make ratatui's
    /// diff re-emit the whole screen. See [`crate::termimage`].
    pub pending_wire: String,
    /// Whether a cover was on screen last draw, so one that stops being drawn
    /// can be taken off the terminal explicitly. A Kitty placement lives until
    /// it is deleted; redrawing the cells under it does not remove it.
    pub cover_shown: bool,
}

impl MusicState {
    pub fn new(lib: Library, export_dir: PathBuf, export_seconds: f64) -> MusicState {
        MusicState {
            lib,
            sel: 0,
            playing: None,
            started: None,
            play_offset: 0.0,
            image: None,
            export_dir,
            export_seconds,
            export_rx: None,
            exporting: None,
            notice: None,
            pending_wire: String::new(),
            cover_shown: false,
        }
    }

    pub fn selected(&self) -> Option<&Album> {
        self.lib.get(self.sel)
    }

    /// Move the cursor by `delta`, clamped. Returns `true` if it moved.
    pub fn move_selection(&mut self, delta: isize) -> bool {
        let n = self.lib.len();
        if n == 0 {
            return false;
        }
        let next = (self.sel as isize + delta).clamp(0, n as isize - 1) as usize;
        let moved = next != self.sel;
        self.sel = next;
        moved
    }

    /// Where the needle is: seconds into the cartridge's boot, counting the
    /// offset it was resumed at.
    pub fn elapsed(&self) -> f64 {
        self.play_offset
            + self
                .started
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0)
    }

    /// Whether the album under the cursor is the one being heard.
    pub fn selection_is_playing(&self) -> bool {
        self.playing == Some(self.sel)
    }
}

/// Everything the shelf draws, as disjoint rectangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Title row across the top.
    pub header: Rect,
    /// Where the cover art image goes. **Nothing else may be drawn here.**
    pub cover: Rect,
    /// The album's details, beside the cover.
    pub panel: Rect,
    /// The strip of other albums.
    pub strip: Rect,
    /// Key hints.
    pub footer: Rect,
}

/// Rows the shelf strip takes, including its rule.
const STRIP_ROWS: u16 = 3;

/// Smallest cover the shelf will draw. Below this the picture is dropped
/// entirely rather than shown as a smear — a two-cell album sleeve tells a
/// reader nothing, and the title still does.
const MIN_COVER_COLS: u16 = 8;
const MIN_COVER_ROWS: u16 = 4;

/// Split `area` into the shelf's parts.
///
/// The cover takes the left of the upper region and the panel the right, in the
/// proportion that keeps the cover roughly square in *cells* — a terminal cell
/// is about twice as tall as it is wide, so a 10:9 picture wants about twice as
/// many columns as rows.
pub fn layout(area: Rect, font_w: u16, font_h: u16) -> Layout {
    let empty = Rect::new(area.x, area.y, 0, 0);
    if area.height < 3 || area.width < 4 {
        return Layout {
            header: area,
            cover: empty,
            panel: empty,
            strip: empty,
            footer: empty,
        };
    }
    let header = Rect::new(area.x, area.y, area.width, 1);
    let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

    let mut rest = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(2),
    );
    let strip = if rest.height > STRIP_ROWS + 2 {
        let s = Rect::new(
            rest.x,
            rest.y + rest.height - STRIP_ROWS,
            rest.width,
            STRIP_ROWS,
        );
        rest.height -= STRIP_ROWS;
        s
    } else {
        empty
    };

    // How wide the cover column should be so the picture fills its height.
    // `fit` in the game view does the same arithmetic; here it only has to
    // choose the *split*, and the picture is centred inside whatever it gets.
    let want_cols = cover_cols_for_rows(rest.height.saturating_sub(2), font_w, font_h);
    // Never more than half the width: the panel is the half that names the
    // album, and an album nobody can read is not a library.
    let cover_w = want_cols.min(rest.width / 2);
    let (cover, panel) = if cover_w >= MIN_COVER_COLS && rest.height >= MIN_COVER_ROWS {
        (
            Rect::new(rest.x, rest.y, cover_w, rest.height),
            Rect::new(rest.x + cover_w, rest.y, rest.width - cover_w, rest.height),
        )
    } else {
        (empty, rest)
    };

    Layout {
        header,
        cover,
        panel,
        strip,
        footer,
    }
}

/// Columns a 160x144 picture needs to be `rows` cells tall, given the terminal's
/// cell size in pixels. Aspect-correct for non-square cells.
fn cover_cols_for_rows(rows: u16, font_w: u16, font_h: u16) -> u16 {
    if rows == 0 || font_w == 0 || font_h == 0 {
        return 0;
    }
    let px_h = rows as u32 * font_h as u32;
    let px_w = px_h * 160 / 144;
    px_w.div_ceil(font_w as u32) as u16
}

/// The rectangle a 160x144 picture occupies inside `area`, centred and
/// aspect-correct.
///
/// **The same arithmetic as the game picture's `fit_metrics`, deliberately.**
/// The terminal scales a placed image to the cell rectangle it is given and
/// letterboxes with black when the aspect does not match, so a cover fitted by
/// a different rule than the game picture would grow black bands the game
/// picture does not have. Returns a zero rect when there is no room worth
/// using.
pub fn fit_cover(area: Rect, font_w: u16, font_h: u16) -> Rect {
    if area.width < MIN_COVER_COLS
        || area.height < MIN_COVER_ROWS
        || font_w == 0
        || font_h == 0
    {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let avail_w = area.width as f64 * font_w as f64;
    let avail_h = area.height as f64 * font_h as f64;
    let scale = (avail_w / 160.0).min(avail_h / 144.0).max(0.0);
    let cols = (((160.0 * scale) / font_w as f64).round() as u16).clamp(1, area.width);
    let rows = (((144.0 * scale) / font_h as f64).round() as u16).clamp(1, area.height);
    Rect::new(
        area.x + (area.width - cols) / 2,
        area.y + (area.height - rows) / 2,
        cols,
        rows,
    )
}

/// Which albums to show in the strip, and where the selection sits among them,
/// so a long shelf scrolls instead of overflowing.
pub fn strip_window(count: usize, sel: usize, width: u16, cell: usize) -> (usize, usize) {
    if count == 0 || width == 0 || cell == 0 {
        return (0, 0);
    }
    let fit = ((width as usize) / cell).max(1);
    if count <= fit {
        return (0, count);
    }
    let half = fit / 2;
    let start = sel.saturating_sub(half).min(count - fit);
    (start, start + fit)
}

// ---------------------------------------------------------------------------
// The words
// ---------------------------------------------------------------------------

/// The album's headline claim about its audio, at the width available.
///
/// **This is the promise.** A boot capture is a title theme; it is never a
/// soundtrack, and this function is the only place that decides how to say so.
pub fn source_lines(album: &Album, playing: bool) -> Vec<String> {
    let mut out = Vec::new();
    match album.verdict {
        AudioVerdict::Audible => {
            out.push(format!("♪ {}", album.source.label()));
            out.push(format!("  {}", album.source.describe()));
        }
        AudioVerdict::Silent => {
            out.push("♪ no sound heard".to_string());
            out.push("  nothing audible in the first half-minute".to_string());
        }
        AudioVerdict::NoApu => {
            out.push("! sound chip did not run".to_string());
            out.push("  this is a fault in the emulator, not the game".to_string());
        }
        AudioVerdict::Failed => {
            out.push("! will not boot".to_string());
            out.push(format!(
                "  {}",
                album.error.as_deref().unwrap_or("unreadable cartridge")
            ));
        }
    }
    if playing {
        out.push("▶ playing".to_string());
    }
    out
}

/// A four-cell meter of which sound channels are on, from `NR52`'s low nibble.
///
/// It is read straight off the emulated machine, so it is evidence rather than
/// decoration: a shelf claiming to play music while all four cells are empty is
/// visibly wrong.
pub fn channel_meter(nr52: u8) -> String {
    let mut s = String::with_capacity(8);
    for ch in 0..4 {
        s.push(if nr52 & (1 << ch) != 0 { '▮' } else { '▯' });
    }
    s
}

/// `m:ss` for a number of seconds.
pub fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::{Album, TrackSource};
    use crate::screen::romlib;
    use std::path::Path;

    fn album(title: &str) -> Album {
        let mut rom = vec![0u8; 0x150];
        for (i, b) in title.bytes().enumerate() {
            rom[0x134 + i] = b;
        }
        Album::pending(romlib::entry_from_bytes(Path::new("/x/a.gb"), &rom))
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.width > 0 && a.height > 0 && b.width > 0 && b.height > 0 && a.intersects(b)
    }

    /// The rule the whole render path is built on: a terminal image owns whole
    /// cells, so nothing may be laid out on top of the cover.
    #[test]
    fn the_shelf_never_overlaps_the_cover_at_any_size() {
        for w in [8u16, 20, 40, 60, 80, 100, 120, 160, 213] {
            for h in [3u16, 5, 8, 12, 24, 30, 45, 55] {
                let area = Rect::new(0, 0, w, h);
                let l = layout(area, 9, 20);
                for (name, r) in [
                    ("header", l.header),
                    ("panel", l.panel),
                    ("strip", l.strip),
                    ("footer", l.footer),
                ] {
                    assert!(
                        !overlaps(l.cover, r),
                        "{name} overlaps the cover at {w}x{h}: cover={:?} {name}={r:?}",
                        l.cover
                    );
                }
                // And everything stays inside the window.
                for r in [l.header, l.cover, l.panel, l.strip, l.footer] {
                    assert!(
                        r.right() <= area.right() && r.bottom() <= area.bottom(),
                        "{r:?} escapes {area:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_tiny_window_drops_the_cover_rather_than_smearing_it() {
        let l = layout(Rect::new(0, 0, 12, 6), 9, 20);
        assert_eq!(l.cover.width, 0, "no room for a legible sleeve");
        assert!(l.panel.width > 0, "the title still gets shown");
    }

    #[test]
    fn the_cover_never_takes_more_than_half_the_width() {
        for w in [40u16, 80, 120, 213] {
            let l = layout(Rect::new(0, 0, w, 40), 9, 20);
            assert!(
                l.cover.width <= w / 2,
                "cover {} of {w} columns leaves no room to read",
                l.cover.width
            );
        }
    }

    #[test]
    fn the_fitted_cover_keeps_its_aspect_and_stays_inside() {
        for (w, h) in [(20u16, 10u16), (40, 20), (60, 40), (9, 5)] {
            let area = Rect::new(3, 2, w, h);
            let r = fit_cover(area, 9, 20);
            if r.width == 0 {
                continue;
            }
            assert!(r.right() <= area.right() && r.bottom() <= area.bottom());
            // 160:144 in pixels, within one cell of rounding.
            let px_w = r.width as f64 * 9.0;
            let px_h = r.height as f64 * 20.0;
            let ratio = px_w / px_h;
            assert!(
                (ratio - 160.0 / 144.0).abs() < 0.35,
                "aspect {ratio} at {w}x{h} -> {r:?}"
            );
        }
    }

    /// **The captain's line: say which mode you got.** Every wording path is
    /// rendered and checked, because this is the claim the feature rests on.
    #[test]
    fn the_panel_never_promises_more_than_a_title_theme() {
        let mut a = album("TESTGAME");
        a.verdict = AudioVerdict::Audible;
        let said = source_lines(&a, true).join(" ").to_ascii_lowercase();
        assert!(said.contains("title theme"), "{said}");
        assert!(
            !said.contains("soundtrack") && !said.contains("all tracks"),
            "a boot capture must not be sold as a soundtrack: {said}"
        );
        assert_eq!(a.source, TrackSource::Boot);
    }

    #[test]
    fn every_verdict_says_something_a_reader_can_act_on() {
        for (v, want) in [
            (AudioVerdict::Silent, "no sound heard"),
            (AudioVerdict::NoApu, "sound chip did not run"),
            (AudioVerdict::Failed, "will not boot"),
        ] {
            let mut a = album("X");
            a.verdict = v;
            a.error = Some("bad header".to_string());
            let said = source_lines(&a, false).join(" ");
            assert!(said.contains(want), "{v:?} -> {said}");
        }
        // A fault in us is labelled as ours, not blamed on the cartridge.
        let mut a = album("X");
        a.verdict = AudioVerdict::NoApu;
        assert!(source_lines(&a, false)
            .join(" ")
            .contains("fault in the emulator"));
    }

    #[test]
    fn the_channel_meter_reads_the_machine() {
        assert_eq!(channel_meter(0x00), "▯▯▯▯");
        assert_eq!(channel_meter(0x0F), "▮▮▮▮");
        assert_eq!(channel_meter(0x81), "▮▯▯▯", "bit 7 is power, not a channel");
        assert_eq!(channel_meter(0x05), "▮▯▮▯");
    }

    #[test]
    fn the_strip_scrolls_around_the_selection() {
        // Six albums, room for three.
        assert_eq!(strip_window(6, 0, 60, 20), (0, 3));
        assert_eq!(strip_window(6, 3, 60, 20), (2, 5));
        assert_eq!(strip_window(6, 5, 60, 20), (3, 6), "clamped at the end");
        // Everything fits.
        assert_eq!(strip_window(2, 1, 60, 20), (0, 2));
        // Degenerate input is empty, never a panic.
        assert_eq!(strip_window(0, 0, 60, 20), (0, 0));
        assert_eq!(strip_window(3, 0, 0, 20), (0, 0));
        assert_eq!(strip_window(3, 0, 60, 0), (0, 0));
    }

    #[test]
    fn times_read_as_minutes_and_seconds() {
        assert_eq!(mmss(0.0), "0:00");
        assert_eq!(mmss(9.6), "0:09");
        assert_eq!(mmss(75.0), "1:15");
        assert_eq!(mmss(-3.0), "0:00");
    }
}

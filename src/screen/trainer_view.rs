//! The trainer panel: how the live Gen 1 state is laid out on screen.
//!
//! Kept apart from [`crate::screen::tui`] so the layout can be unit-tested
//! without a terminal, and so the two rules the render work paid for stay
//! visible:
//!
//! * **Text and the picture never share a cell.** A terminal graphics image
//!   owns whole cells; text drawn into them replaces that part of the image, and
//!   with Kitty's Unicode placeholders it stops the picture updating at all.
//!   [`layout`] therefore returns disjoint rectangles, and
//!   `the_trainer_panel_never_overlaps_the_picture_at_any_size` asserts it.
//! * **The sprite is itself an image**, so it gets its own rectangle and its own
//!   image id, and the same rule applies to it.
//!
//! # The shape of the panel
//!
//! **A left sprite gutter, and facts in columns to the right of it.** One
//! grammar, applied to every block: wherever the panel names a Pokémon, its
//! picture is in the gutter on that block's own rows and everything said about
//! it is aligned in columns beside it. Sections are announced by a
//! **left-anchored rule** ([`crate::screen::panel::rule`]) so every section
//! label starts in the same column and the page can be scanned down its edge.
//!
//! ```text
//! ┌ Trainer · d detail · Tab ────────────┐
//! │BLUE  ID 62800  ¥3,000                │
//! │0:01:41  Pokémon Blue                 │
//! │●●○○ ○○○○ 2/8  next Lt.Surge          │
//! │─ OPPONENT ─────────────── JOEY ──────│
//! │▟▙   CHARMANDER            L7  FIRE   │
//! │▛▜   ███████░░░ 18/20   ATK-1 SPD+2   │
//! │▘▝   ATK 42  DEF 38  SPD 61  SPC 50   │
//! │─ PARTY ──────────────────────── 4/6 ─│
//! │▟▙  ▸1 SPARK      L12 ██████░░ PSN ★  │
//! │▛▜     PIKACHU  34/ 34  ELECTRIC      │
//! │▟▙   2 RATTY     L 9 ███░░░░░ PSN     │
//! │▛▜     RATTATA  11/ 27  NORMAL        │
//! └──────────────────────────────────────┘
//! ```
//!
//! The `▟▙`/`▛▜` blocks stand for the sprite images, which are **not text** —
//! see "Pictures" below.
//!
//! ## What changed, and why
//!
//! Three things the maintainer asked for, against the panel this replaces:
//!
//! * **Every party member is visible at all times, with its own picture.** The
//!   party used to be two lines of text per Pokémon and *nothing* in the detail
//!   view, so opening `d` lost the team. The team is now the spine of the
//!   panel: it is in both modes, and [`layout_with_pictures`] gives up the
//!   focused block's own picture before it gives up a party row.
//! * **The sprites are small.** One picture used to take eleven of a
//!   twenty-four-row panel. A party cell is [`SLOT_ROWS`] rows and the focused
//!   block's is [`FOCUS_ROWS`]; nothing on the panel is bigger than the text
//!   beside it.
//! * **Chrome earns its row or goes.** `[ TRAINER ]`, `[ BADGES ]` and the
//!   `BADGES`/`NEXT` footer are gone — the trainer's own two lines say what
//!   they said, and the badge row carries the next gym. `[ MOVES ]` and
//!   `[ HIDDEN ]` became column headers, so those rows are legends rather than
//!   labels. Six rows of decoration became one row of data and five rows of
//!   party.
//!
//! # Degrading as the panel narrows and shortens
//!
//! Every section takes the width it is given and drops its **least valuable
//! column first** rather than wrapping into mush. The party rows have four
//! tiers ([`party_cols`]); the moves table has three ([`MoveCols`]); the
//! trainer fields drop by a declared priority ([`Field::drop`]); the badge row
//! falls from `[●][●]…` to `●●…` to a bare count.
//! `no_line_overflows_at_any_width` renders every section at every width from 8
//! to 60 and asserts nothing exceeds it — the layout is guaranteed by that
//! test, not by arithmetic in a comment.
//!
//! Vertically the order of sacrifice is declared in [`layout_with_pictures`]
//! and it is the same principle: **the pictures are the decoration and the six
//! slots are the point.** The focused block's picture goes first, then the
//! party pictures (the rows stay, they just lose their gutter), then the party
//! collapses to one line per Pokémon, and only after all of that does anything
//! fall off the bottom.
//!
//! # Pictures
//!
//! Both gutters are **terminal images**, not text, so the same three rules
//! apply to them as to the game picture (`docs/terminal-render.md`): one image
//! id reused for the life of the session, a cell rectangle text never shares,
//! and nothing transmitted for an unchanged picture. The party gutter is
//! **one** composed image of six cells rather than six images — that is what
//! makes six small sprites cheaper than the single large one they replace, and
//! `render_trainer` keys it on the party's species and fainted-ness so it is
//! recomposed when a Pokémon changes and not once a frame.
//!
//! A terminal with no image protocol draws both gutters through
//! [`crate::screen::halfblocks`] instead. It is the same fallback the game
//! picture and the music-mode sleeve already take, and without it the whole
//! feature is invisible in every VTE-family terminal.
//!
//! # Glyphs
//!
//! Only characters already used elsewhere in this frontend: box drawing
//! (`─ │ ┌`), the block elements `█ ▌ ░`, the geometric shapes `● ○ ▸ ▲ ▼`, and
//! `★`.
//! All of them come from fonts a terminal already needs for the rest of the UI
//! — nothing here wants a Nerd Font, and nothing is double-width.
//!
//! # What earns space, and what is behind a key
//!
//! Taken from the [Ironmon Tracker](https://github.com/besteon/Ironmon-Tracker)
//! (MIT; read for its choices, not its code — it is Lua against a different
//! emulator and targets Gen 3):
//!
//! * A **compact default** that fits at a glance and a **detail view on
//!   demand**, rather than one dense screen. The tracker shows one Pokémon's
//!   essentials plus its moves and hides everything else behind screens.
//! * **HP is colour-coded by fraction** — green, then amber under a half, then
//!   red under a fifth. It is the fastest thing to read on the panel and costs
//!   nothing.
//! * **A single "unknown" glyph.** The tracker uses `?` everywhere for
//!   not-yet-known data; a blank reads as "zero" and a zero is a lie. Same here:
//!   [`UNKNOWN`].
//!
//! Deliberately different for Gen 1: there are no abilities, natures or held
//! items to show, "special" is one stat rather than two, and DVs go to 15 rather
//! than IVs to 31 — so the detail view spends its space on DVs, the invisible
//! stat experience, and the derived values in [`terminalgb_plugin_trainer::derived`].

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::screen::panel;
use terminalgb_plugin_trainer::encounter::WildEncounter;
use terminalgb_plugin_trainer::gen1::{BattleKind, BattleType};
use terminalgb_plugin_trainer::gen1::{Mon, MoveSlot, BADGE_LEADERS};
use terminalgb_plugin_trainer::{TeamMon, TrainerState};

/// Shown wherever a value is not known yet, so a blank never reads as a zero.
pub const UNKNOWN: &str = "?";

/// Width of the trainer panel in cells. Two border columns plus the 38 the
/// party rows are laid out for — a name, a level, an eight-cell HP bar, the
/// status and the shiny mark, with the species and both types under them.
pub const PANEL_W: u16 = 40;

/// Narrowest panel worth drawing at all. Below this the party rows lose the
/// level as well as the bar and the header keeps only a name, which is less
/// information than the space costs — so the frontend gives the width back to
/// the game picture instead.
pub const PANEL_MIN_W: u16 = 22;

/// Rows one party slot occupies when it has a picture: a headline row and a
/// reference row, with the sprite cell spanning both.
///
/// Two is the smallest a Pokémon stays identifiable at — one row is sixteen
/// device pixels on a normal cell, which is a smudge — and it is also exactly
/// what the two text lines need, so the gutter and the text divide the same
/// rows with nothing left over.
pub const SLOT_ROWS: u16 = 2;

/// Rows the focused block (the opponent in a battle, or `d`'s subject) gives
/// its own picture. One more than a party cell, because it is the thing the
/// panel is currently *about*.
pub const FOCUS_ROWS: u16 = 3;

/// What kind of picture this terminal can draw, which decides how many cells
/// a picture needs to be worth drawing at all.
///
/// **This is not a cosmetic distinction.** An image-protocol cell carries
/// 8x16 device pixels; a half-block cell carries 1x2 samples. So the same
/// icon needs roughly eight times the cells in each axis — and a party slot
/// is two rows, which is four half-block samples tall. Measured: a Pokémon at
/// five columns by four samples is noise, not a Pokémon. Half-blocks
/// therefore get **no party gutter** and a **larger focused picture**, which
/// is the one that has the rows to be legible, and the panel says so rather
/// than drawing six smudges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pictures {
    /// No pictures at all — the message screens, and a panel too small.
    None,
    /// Drawn from `▀`, so a cell is two samples. See
    /// [`crate::screen::halfblocks`].
    Halfblocks,
    /// A terminal image protocol: a cell is a font cell of real pixels.
    Image,
}

impl Pictures {
    /// Rows the focused block gives its own picture.
    pub fn focus_rows(self) -> u16 {
        match self {
            Pictures::None => 0,
            // Twelve samples tall: enough for a silhouette, and the rows come
            // out of the block's own text, which needs three of them anyway.
            Pictures::Halfblocks => 6,
            Pictures::Image => FOCUS_ROWS,
        }
    }

    /// Whether the party's six slots each get a picture.
    pub fn party(self) -> bool {
        self == Pictures::Image
    }

    /// Gutter width in columns at a panel `inner` wide, or 0 for a panel too
    /// narrow to spend any.
    pub fn gutter(self, inner: u16) -> u16 {
        match self {
            Pictures::None => 0,
            Pictures::Halfblocks if inner < HALFBLOCK_MIN_PANEL_W => 0,
            // Wider, because a half-block column is one sample rather than
            // eight pixels — and affordable, because only one block has a
            // picture rather than seven.
            Pictures::Halfblocks => 10,
            Pictures::Image => gutter_w(inner),
        }
    }
}

/// Narrowest panel that spends columns on a half-block picture. Wider than
/// [`GUTTER_MIN_PANEL_W`] because the picture itself has to be wider.
pub const HALFBLOCK_MIN_PANEL_W: u16 = 34;

/// What a terminal with no image protocol is told about the party pictures it
/// is not getting.
///
/// One line, in the panel, rather than a silent absence — the rule the rest of
/// this frontend already follows for a missing capability
/// (`BASIC_INPUT_NOTICE`, `HALFBLOCKS_NOTICE`). Six unrecognisable smudges
/// would be worse than the sentence.
pub const NO_PARTY_PICTURES: &str = "party pictures need an image protocol";

/// Columns the sprite gutter takes at the panel's design width.
///
/// Sized so a party cell is close to square in device pixels at an ordinary
/// 8x16 cell: five columns by two rows is 40x32. The composer letterboxes the
/// sprite into whatever the gutter actually turns out to be, so this is a
/// budget rather than an assumption about anybody's font.
pub const GUTTER_W: u16 = 5;

/// Columns of gutter at a given panel width, or 0 for a panel too narrow to
/// spend any.
///
/// Below [`GUTTER_MIN_PANEL_W`] the gutter is refused outright rather than
/// shrunk: three columns of Pokémon is a smudge, and the row it sits on needs
/// the space more.
pub fn gutter_w(inner: u16) -> u16 {
    if inner < GUTTER_MIN_PANEL_W {
        0
    } else if inner < 34 {
        4
    } else {
        GUTTER_W
    }
}

/// Narrowest panel that spends any columns on pictures.
pub const GUTTER_MIN_PANEL_W: u16 = 28;

/// Rows the trainer header always takes: the two trainer lines and the badge
/// row. No section rule — the header is the top of the panel and a label
/// saying so would be a row spent restating the border's own title.
pub const HEADER_ROWS: u16 = 3;

/// Rows of text that must survive before *any* picture is allowed one. The
/// party is the point; the pictures are the decoration.
const MIN_BODY_ROWS: u16 = 6;

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Where the panel's pieces go inside its rectangle.
///
/// Deliberately only three: a header, one body of text, and a **gutter width**
/// reserved on the body's left for pictures. The pictures are not rectangles
/// in this struct because they do not have rows of their own — a sprite spans
/// exactly the rows the text beside it already needed
/// ([`PanelLayout::gutter_rect`] cuts one out on demand). That is the whole
/// reason six party pictures are affordable where one large one was not: they
/// cost columns, never rows.
pub struct PanelLayout {
    /// The single bordered block, drawn over the whole panel rectangle.
    pub frame: Rect,
    /// The trainer header: name, ID, money, play time, badges, next gym.
    pub header: Rect,
    /// Everything else, as one paragraph. Composed lines are already indented
    /// past [`PanelLayout::gutter`], so text and picture never share a cell.
    pub body: Rect,
    /// Columns reserved on the left of [`body`](PanelLayout::body) for the
    /// sprite gutters. Zero on a panel too narrow to spend them, or in a
    /// terminal that cannot draw a picture at all.
    pub gutter: u16,
}

impl PanelLayout {
    /// The rectangle a picture occupies: `rows` rows of gutter, starting
    /// `row` rows into the body.
    ///
    /// `None` when there is no gutter, or when the rows asked for have fallen
    /// off the bottom of the body — a picture whose text has been clipped away
    /// must go with it rather than hang over the next section.
    pub fn gutter_rect(&self, row: u16, rows: u16) -> Option<Rect> {
        if self.gutter == 0 || rows == 0 || row + rows > self.body.height {
            return None;
        }
        Some(Rect::new(self.body.x, self.body.y + row, self.gutter, rows))
    }

    /// Shrink the panel to the rows it actually uses and **centre it
    /// vertically** in `area`, so it sits at the same height as the game
    /// picture beside it rather than starting at the ceiling.
    ///
    /// `fit_metrics` already centres the picture in its own column, so a
    /// top-aligned panel is what made the two columns read as unrelated things
    /// pasted side by side: the picture floating in the middle of one, the
    /// panel jammed to the top of the other, and a different-sized gap under
    /// each. Centring both makes them one composition.
    ///
    /// When the content is taller than the column there is no slack to
    /// distribute, and the panel stays exactly where it was: full height and
    /// top-aligned, so what falls off is the **bottom**. Centring an
    /// over-height panel would push its header off the top, and the header is
    /// the part that says whose panel this is.
    pub fn centre_in(self, area: Rect, body_rows: u16) -> PanelLayout {
        let content = body_rows.min(self.body.height);
        let used = 2 + self.header.height + content;
        if used >= area.height {
            return self;
        }
        let dy = (area.height - used) / 2;
        PanelLayout {
            frame: Rect::new(area.x, area.y + dy, area.width, used),
            header: Rect::new(
                self.header.x,
                self.header.y + dy,
                self.header.width,
                self.header.height,
            ),
            body: Rect::new(self.body.x, self.body.y + dy, self.body.width, content),
            gutter: self.gutter,
        }
    }
}

/// Split the panel rectangle.
///
/// `pictures` is what this terminal can draw — see [`Pictures`], which owns
/// the sizing. It only ever changes the gutter's *width*; it never moves a
/// row, so a panel with pictures and one without lay their text out
/// identically apart from the indent.
pub fn layout(area: Rect, pictures: Pictures) -> PanelLayout {
    // Inside the border. A panel too small to have an interior gets zero-sized
    // pieces rather than rectangles that escape it.
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let header_h = HEADER_ROWS.min(inner.height);
    let body_h = inner.height.saturating_sub(header_h);
    let gutter = if body_h >= MIN_BODY_ROWS {
        pictures.gutter(inner.width)
    } else {
        0
    };
    PanelLayout {
        frame: area,
        header: Rect::new(inner.x, inner.y, inner.width, header_h),
        body: Rect::new(inner.x, inner.y + header_h, inner.width, body_h),
        gutter,
    }
}

/// [`layout`], with the gutter withdrawn when the composed body did not ask
/// for one.
///
/// The party's pictures need two rows per slot; when the panel is too short
/// for that the party collapses to one line each and the gutter goes with it,
/// because a one-row sprite is sixteen device pixels of smudge. Passing the
/// composed body back in is what lets that decision be made once, by the code
/// that already knows which tier it chose, rather than guessed at twice.
pub fn layout_with_pictures(area: Rect, body: &Body) -> PanelLayout {
    let mut l = layout(area, body.pictures);
    // The composer already decided; the layout only places what it asked for.
    l.gutter = body.gutter;
    l
}

/// The panel's single frame.
pub fn panel_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(dim())
}

/// A bordered block for the panel's *messages* — the unsupported-cartridge and
/// waiting-for-a-save screens, which are prose rather than sections.
pub fn body_block(title: &str) -> Block<'_> {
    panel_block(title)
}

/// Clip a composed line to `width`. See [`panel::clamp_line`].
pub fn clamp_line(l: Line<'_>, width: usize) -> Line<'_> {
    panel::clamp_line(l, width)
}

/// A left-anchored section rule. See [`panel::rule`].
pub fn rule<'a>(title: &str, width: usize) -> Line<'a> {
    panel::rule(title, width)
}

/// A left-anchored section rule with a right-aligned annotation.
pub fn rule_with<'a>(title: &str, note: &str, width: usize) -> Line<'a> {
    panel::rule_with(title, Some(note), width)
}

/// A plain full-width separator, used where a section break needs no name.
pub fn separator<'a>(width: usize) -> Line<'a> {
    Line::from(Span::styled("─".repeat(width), dim()))
}

/// One labelled field on a packed line.
struct Field {
    label: &'static str,
    value: String,
    /// Sacrifice order when the line does not fit: 0 never goes, then 1, 2, …
    /// The *most* droppable field goes first.
    drop: u8,
}

impl Field {
    /// A label is followed by one space; an *empty* label is not, or the value
    /// arrives indented by a space that looks like a typo (`¥ 3,000`).
    fn prefix(&self) -> String {
        if self.label.is_empty() {
            String::new()
        } else {
            format!("{} ", self.label)
        }
    }

    fn width(&self) -> usize {
        self.prefix().chars().count() + self.value.chars().count()
    }
}

/// Join `fields` with ` │ `, dropping the most-droppable until the line fits.
///
/// Labels are dim and values are not, so the eye lands on the value; that is
/// most of what makes a packed line readable at a glance rather than a wall.
fn fields_line<'a>(mut fields: Vec<Field>, width: usize) -> Line<'a> {
    let total = |f: &Vec<Field>| -> usize {
        f.iter().map(Field::width).sum::<usize>() + 3 * f.len().saturating_sub(1)
    };
    while total(&fields) > width && fields.len() > 1 {
        let Some((i, _)) = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.drop > 0)
            .max_by_key(|(_, f)| f.drop)
        else {
            break;
        };
        fields.remove(i);
    }
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            if used + 3 > width {
                break;
            }
            // A middle dot rather than a bar: with most of these fields now
            // unlabelled, a full-height rule between them reads as a table
            // border and makes the header the heaviest thing on the panel.
            spans.push(Span::styled(" · ", panel::label()));
            used += 3;
        }
        let label = f.prefix();
        let lw = label.chars().count();
        if used + lw >= width {
            break;
        }
        if lw > 0 {
            spans.push(Span::styled(label, dim()));
            used += lw;
        }
        let value = clip(&f.value, width - used);
        used += value.chars().count();
        spans.push(Span::styled(value, panel::value()));
    }
    Line::from(spans)
}

/// HP colour by remaining fraction. See [`panel::hp_color`].
pub fn hp_color(fraction: f32) -> Color {
    panel::hp_color(fraction)
}

/// A fixed-width HP bar. See [`panel::bar`], which owns the rule that a
/// survivor never renders as an empty bar.
pub fn hp_bar(fraction: f32, width: usize) -> String {
    panel::bar(fraction, width)
}

/// The cable, as one line for the top of the trainer panel.
///
/// The panel is what a player actually watches, and the question it has to
/// answer is "is there anybody to trade with?" — so it distinguishes the three
/// states that change the answer: **nobody connected**, **someone connected**,
/// and **an exchange happening right now**. Anything more is clutter in a panel
/// that is already dense.
///
/// `summary` comes from the frontend's single `link_summary`, so this and the
/// Status row cannot disagree. Degrades by the same `clamp_line` every other
/// row uses, so a narrow split loses the tail of the text and never the label.
pub fn link_line<'a>(summary: &str, active: bool, width: usize) -> Line<'a> {
    let style = if active {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if summary.starts_with("connected") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    clamp_line(
        Line::from(vec![
            Span::styled("LINK ", Style::default().fg(Color::DarkGray)),
            Span::styled(summary.to_string(), style),
        ]),
        width,
    )
}

pub fn header_lines<'a>(state: &TrainerState, game: &str, width: usize) -> Vec<Line<'a>> {
    header_rows(state, game, width)
        .into_iter()
        .map(|l| clamp_line(l, width))
        .collect()
}

fn header_rows<'a>(state: &TrainerState, game: &str, width: usize) -> Vec<Line<'a>> {
    let (h, m, s) = state.play_time;
    vec![
        // The trainer's name is the value, not a labelled field: it is the
        // only string on the row that is not self-describing, and `NAME `
        // spent five of thirty-eight columns saying so.
        fields_line(
            vec![
                Field {
                    label: "",
                    value: state.player_name.clone(),
                    drop: 0,
                },
                // The ID is the field a run refers to least often, so it is the
                // first to go when the panel is narrow.
                Field {
                    label: "ID",
                    value: format!("{:05}", state.player_id),
                    drop: 2,
                },
                Field {
                    label: "",
                    value: format!("\u{a5}{}", thousands(state.money)),
                    drop: 1,
                },
            ],
            width,
        ),
        fields_line(
            vec![
                Field {
                    label: "",
                    value: format!("{h}:{m:02}:{s:02}"),
                    drop: 0,
                },
                Field {
                    label: "",
                    value: game.to_string(),
                    drop: 1,
                },
            ],
            width,
        ),
        badge_row(state, width),
    ]
}

/// Eight discrete badge markers in two groups of four, plus the count.
///
/// Filled and empty differ in **both** glyph and colour (`●` yellow against `○`
/// dark grey), because a colour-blind reader or a low-contrast theme must still
/// be able to count them.
pub fn badge_row<'a>(state: &TrainerState, width: usize) -> Line<'a> {
    let flags = state.badge_flags();
    let count = format!("{}/8", state.badge_count());
    let got = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    // **The next gym rides on this row rather than in a footer of its own.**
    // It used to cost two rows at the bottom of the panel — a `BADGES` list
    // that restated these very markers, and a `NEXT` line — and it is the one
    // part of that footer a run acts on. The names of the badges already held
    // went with the footer: the markers say how many and which, in order.
    let next = BADGE_LEADERS
        .iter()
        .zip(flags)
        .find(|(_, g)| !*g)
        .map(|(n, _)| *n);
    // Two forms, because on a narrow panel the *leader's name* is the fact and
    // the word "next" is not.
    let (tail, short) = match next {
        Some(leader) => (format!("  next {leader}"), format!("  \u{2192}{leader}")),
        None => ("  all eight".to_string(), "  8/8".to_string()),
    };

    // `[●][●][●][●] [●][●][●][●]  n/8`
    let bracketed = 8 * 3 + 1 + 2 + count.chars().count();
    // `●●●● ●●●●  n/8`
    let bare = 8 + 1 + 2 + count.chars().count();
    let tail_w = tail.chars().count();
    // **The brackets are the first thing to go, not the next gym.** They are
    // decoration around markers that already differ in glyph *and* colour, and
    // thirteen columns of them is exactly what stops the actionable half of
    // the old footer fitting on this row at the design width.
    let boxed = width >= bracketed + tail_w;

    let mut spans = Vec::new();
    if boxed {
        for (i, &on) in flags.iter().enumerate() {
            if i == 4 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled("[", panel::label()));
            spans.push(if on {
                Span::styled("●", got)
            } else {
                Span::styled("○", panel::label())
            });
            spans.push(Span::styled("]", panel::label()));
        }
    } else if width >= bare {
        for (i, &on) in flags.iter().enumerate() {
            if i == 4 {
                spans.push(Span::raw(" "));
            }
            spans.push(if on {
                Span::styled("●", got)
            } else {
                Span::styled("○", panel::label())
            });
        }
    } else {
        // No room for markers at all: the count is the part that cannot go.
        return Line::from(Span::styled(clip(&count, width), got));
    }
    spans.push(Span::styled(format!("  {count}"), panel::label()));
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let style = if next.is_some() { panel::label() } else { got };
    if used + tail_w <= width {
        spans.push(Span::styled(tail, style));
    } else if used + short.chars().count() <= width {
        spans.push(Span::styled(short, style));
    }
    Line::from(spans)
}

/// The six party slots as markers: filled slots coloured by how much HP is
/// left, empty slots hollow and dim, a fainted Pokémon hollow and red.
///
/// This is the row that answers "how is the team doing" without reading a
/// single number, and it is the only place an *empty* slot is visible at all —
/// the party list below only lists what exists. `active` is the party index the
/// battle engine currently has out, which is underlined.
pub fn slot_row<'a>(
    state: &TrainerState,
    active: Option<usize>,
    width: usize,
) -> Line<'a> {
    let mut spans = Vec::new();
    let n = terminalgb_plugin_trainer::gen1::PARTY_LENGTH;
    let text_w = n * 2 - 1;
    let pad = width.saturating_sub(text_w) / 2;
    if width < text_w {
        return Line::from(Span::raw(""));
    }
    spans.push(Span::raw(" ".repeat(pad)));
    for i in 0..n {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let mut style = match state.party.get(i) {
            None => dim(),
            Some(m) => Style::default().fg(hp_color(m.hp_fraction())),
        };
        if active == Some(i) {
            style = style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
        }
        let glyph = match state.party.get(i) {
            // A hollow marker reads as "gone", which is what a fainted slot is;
            // a filled red one would be indistinguishable from one on 1 HP.
            Some(m) if m.is_fainted() => "○",
            Some(_) => "●",
            None => "○",
        };
        spans.push(Span::styled(glyph, style));
    }
    Line::from(spans)
}

/// What the sprite and the detail view are pointed at.
///
/// The panel used to have no such concept: the *sprite* switched to the
/// opponent during a battle while the numbers under it stayed the player's
/// party, so the picture and the statistics described different Pokémon. That
/// is worse than showing nothing — a tracker exists to be read before
/// committing to a turn. One focus now drives both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Opponent,
    Party(usize),
}

impl Focus {
    pub fn is_opponent(self) -> bool {
        matches!(self, Focus::Opponent)
    }
}

/// The ring `Tab` walks.
///
/// **The opponent is first while a battle is on**, and the party follows it, so
/// the thing that is about to hit you is one keypress from its full stat block
/// and the team you would switch to is the next few. Out of battle the ring is
/// just the party.
pub fn focus_ring(state: &TrainerState) -> Vec<Focus> {
    let mut ring = Vec::with_capacity(state.party.len() + 1);
    if state.opponent.is_some() && state.battle != BattleKind::None {
        ring.push(Focus::Opponent);
    }
    ring.extend((0..state.party.len()).map(Focus::Party));
    ring
}

/// Resolve a ring position to a focus, clamping a stale index rather than
/// vanishing — the ring changes length the instant a battle starts or ends.
pub fn focus_at(state: &TrainerState, sel: usize) -> Option<Focus> {
    let ring = focus_ring(state);
    if ring.is_empty() {
        return None;
    }
    Some(ring[sel.min(ring.len() - 1)])
}

/// The Pokémon a focus names.
pub fn focused_mon(state: &TrainerState, focus: Focus) -> Option<&Mon> {
    match focus {
        Focus::Opponent => state.opponent.as_ref(),
        Focus::Party(i) => state.party.get(i),
    }
}

/// The column widths a party row uses at a given panel width.
///
/// Four tiers rather than continuous arithmetic, so every row in a panel makes
/// the *same* decisions and the columns line up down the page. What goes first
/// is what a run reads least: the second line's type label, then the status
/// column, then the HP bar's resolution, and only at the very end the bar
/// itself. `no_line_overflows_at_any_width` proves each tier fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartyCols {
    /// Nickname / species column.
    pub nick: usize,
    /// HP bar cells; 0 for no bar at all.
    pub bar: usize,
    /// Whether the three-character status column is drawn.
    pub status: bool,
    /// Type column; 0 for no type.
    pub ty: usize,
}

pub fn party_cols(width: usize) -> PartyCols {
    // Above the design width the columns *grow* rather than stranding the extra
    // space: longer names survive untruncated and the HP bar gains resolution,
    // which is the only honest way text "scales" on a cell grid. Thirds of the
    // width, so the three columns stay in proportion to each other and to the
    // panel however wide it gets.
    if width >= 44 {
        let third = width / 3;
        return PartyCols {
            nick: third.min(18),
            bar: third.min(20),
            status: true,
            ty: third.min(16),
        };
    }
    // The thresholds are the width **left of the sprite gutter**, not the
    // panel's: the gutter takes its columns before the party row is laid out,
    // so a 40-column panel lays its rows out for 33 and the top tier has to
    // start below that or the design width would fall into the tier under it.
    if width >= 32 {
        PartyCols {
            nick: 10,
            bar: 8,
            status: true,
            ty: 12,
        }
    } else if width >= 27 {
        PartyCols {
            nick: 8,
            bar: 5,
            status: true,
            ty: 8,
        }
    } else if width >= 21 {
        PartyCols {
            nick: 7,
            bar: 3,
            status: false,
            ty: 3,
        }
    } else {
        PartyCols {
            nick: width.saturating_sub(11).max(3),
            bar: 0,
            status: false,
            ty: 0,
        }
    }
}

/// The first line of a party entry: marker, index, nickname, level, HP bar,
/// status, shiny mark.
fn party_head<'a>(
    i: usize,
    m: &Mon,
    selected: bool,
    active: bool,
    c: PartyCols,
    width: usize,
) -> Line<'a> {
    let frac = m.hp_fraction();
    // Every column is pushed only if it fits, in decreasing order of value.
    // The tiers in `party_cols` are the *intent*; this is what makes the row
    // safe at a width no tier anticipated.
    let nick = c.nick.min(width.saturating_sub(3));
    // One gutter cell, two meanings that never both apply to the reader at
    // once: `▸` is what `d` will open, `▌` is the Pokémon currently in the
    // fight. Focus wins the cell, because the picture beside it is the
    // focused one too.
    let gutter = if selected {
        "▸"
    } else if active {
        "▌"
    } else {
        " "
    };
    let mut spans = vec![
        Span::styled(
            format!("{gutter}{} ", i + 1),
            if active {
                Style::default().fg(panel::ink::ACCENT)
            } else {
                dim()
            },
        ),
        Span::styled(
            pad(&m.nickname, nick),
            Style::default().add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
    ];
    let mut used = 3 + nick;
    if used + 4 > width {
        return Line::from(spans);
    }
    spans.push(Span::styled(format!(" L{:>3}", m.level), panel::value()));
    used += 4;
    if c.bar > 0 && used + 1 + c.bar <= width {
        spans.push(Span::raw(" "));
        spans.push(panel::hp_bar_span(frac, c.bar));
        used += 1 + c.bar;
    }
    if c.status && used + 4 <= width {
        let label = m
            .status
            .label()
            .unwrap_or(if m.is_fainted() { "FNT" } else { "" });
        // Coloured by *which* status, not by "something is wrong": a burn, a
        // paralysis and a sleep each want a different decision, and the panel
        // can say which before the word has been read.
        spans.push(Span::styled(
            format!(" {label:<3}"),
            Style::default().fg(panel::status_color(label)),
        ));
        used += 4;
    }
    if m.would_be_shiny() && used < width {
        // A space before it where there is room: `PSN★` reads as one token,
        // and the star is the loudest thing on the row precisely because it
        // is separate from everything else.
        let mark = if used + 2 <= width { " ★" } else { "★" };
        spans.push(Span::styled(mark, Style::default().fg(panel::ink::SHINY)));
    }
    Line::from(spans)
}

/// The second line of a party entry: species, HP numbers, types. Dim, because
/// it is the reference half — the eye should land on the line above.
fn party_body<'a>(m: &Mon, c: PartyCols, width: usize) -> Line<'a> {
    let nick = c.nick.min(width.saturating_sub(3));
    let mut spans = vec![Span::styled(format!("   {}", pad(&m.name, nick)), dim())];
    let mut used = 3 + nick;
    if used + 8 > width {
        return Line::from(spans);
    }
    spans.push(Span::styled(format!(" {:>3}/{:>3}", m.hp, m.max_hp), dim()));
    used += 8;
    if c.ty > 0 && used + 1 < width {
        let room = c.ty.min(width - used - 1);
        spans.push(Span::raw(" "));
        spans.extend(type_spans(m, room));
    }
    Line::from(spans)
}

/// A Pokémon's type or types, **each in its own colour**, fitted to `max`
/// columns.
///
/// Two spans rather than one string because a dual type is two facts: colouring
/// `GRASS/POISON` uniformly says "this is a type label", colouring each half
/// says which two. Falls back to three-letter abbreviations, which stay
/// readable where a truncation (`GRASS/POI…`) does not — and the abbreviations
/// keep their colours, because the colour matters most exactly where the text
/// had to be shortened.
fn type_spans<'a>(m: &Mon, max: usize) -> Vec<Span<'a>> {
    let (a, b) = (m.types.0.clone(), m.types.1.clone());
    let dual = !b.is_empty() && a != b;
    let full = if dual { format!("{a}/{b}") } else { a.clone() };
    let short = |t: &str| -> String { t.chars().take(3).collect() };
    let (a, b) = if full.chars().count() <= max {
        (a, b)
    } else {
        (short(&a), short(&b))
    };
    if !dual {
        return vec![Span::styled(
            clip(&a, max),
            Style::default().fg(panel::type_color(&a)),
        )];
    }
    let joined = format!("{a}/{b}");
    if joined.chars().count() > max {
        // Not even the abbreviations fit: keep the first type whole rather
        // than half of each.
        return vec![Span::styled(
            clip(&a, max),
            Style::default().fg(panel::type_color(&a)),
        )];
    }
    vec![
        Span::styled(a.clone(), Style::default().fg(panel::type_color(&a))),
        Span::styled("/", dim()),
        Span::styled(b.clone(), Style::default().fg(panel::type_color(&b))),
    ]
}

/// The party list: one entry per Pokémon, indented past the sprite gutter.
///
/// `compact` collapses the two lines to one, which is the tier a short panel
/// takes — and with one row a slot the gutter goes too, because a one-row
/// sprite is a smudge. `gutter` is the indent, already decided by the layout.
pub fn party_lines<'a>(
    state: &TrainerState,
    selected: Option<usize>,
    width: usize,
    compact: bool,
) -> Vec<Line<'a>> {
    party_lines_indented(state, selected, width, compact, 0)
}

/// [`party_lines`] with an explicit gutter indent.
pub fn party_lines_indented<'a>(
    state: &TrainerState,
    selected: Option<usize>,
    width: usize,
    compact: bool,
    gutter: usize,
) -> Vec<Line<'a>> {
    if state.party.is_empty() {
        return vec![Line::from(Span::styled("  no Pokémon yet", dim()))];
    }
    // The text lives to the right of the gutter, so it is laid out for the
    // columns it actually has.
    let gutter = text_indent(gutter);
    let text_w = width.saturating_sub(gutter);
    let c = party_cols(text_w);
    let active = active_slot(state);
    let mut lines = Vec::new();
    for (i, m) in state.party.iter().enumerate() {
        lines.push(indent(
            party_head(i, m, selected == Some(i), active == Some(i), c, text_w),
            gutter,
        ));
        if !compact {
            lines.push(indent(party_body(m, c, text_w), gutter));
        }
    }
    lines
}

/// Columns of text indent a `g`-column picture gutter costs.
///
/// One more than the picture, so a sprite and the word beside it are not
/// touching. The gutter *rectangle* stays `g` wide — the extra column is
/// blank text, not picture — which is what keeps the "text and the picture
/// never share a cell" rule true with room to spare.
pub fn text_indent(g: usize) -> usize {
    if g == 0 {
        0
    } else {
        g + 1
    }
}

/// Indent every line of `beside` past a `g`-column gutter and **pad it to
/// `rows`**, so a picture of exactly `rows` rows can be placed over it.
///
/// The padding is the load-bearing half. A half-block picture needs twice the
/// rows of an image one to be legible, so it can want more rows than the
/// identity lines beside it have — and without padding it would reach down
/// into whatever comes next, which is a *table*. A moves table whose header is
/// indented and whose rows are not is a legend that lies about its own
/// columns; it was drawn exactly once, in a screenshot, and this is the fix.
///
/// A blank row beside a picture is not waste: it **is** the picture.
fn indent_beside(beside: &mut Vec<Line<'_>>, g: usize, rows: usize) {
    if g == 0 || rows == 0 {
        return;
    }
    let pad = " ".repeat(text_indent(g));
    for l in beside.iter_mut().take(rows) {
        let taken = std::mem::take(&mut l.spans);
        let mut spans = Vec::with_capacity(taken.len() + 1);
        spans.push(Span::raw(pad.clone()));
        spans.extend(taken);
        l.spans = spans;
    }
    // Pad rather than let the picture reach the row below. Only ever needed
    // for the half-block picture, which is the taller of the two.
    while beside.len() < rows {
        beside.push(Line::from(Span::raw(pad.clone())));
    }
}

/// The width a block's row `i` has, given a picture covering its first `rows`
/// rows of a `g`-column gutter.
///
/// A row the picture covers is laid out for what is left of the panel; a row
/// below it gets the whole panel back. Getting this wrong is not cosmetic —
/// it is how the opponent's DV legend came to be measured against columns the
/// picture had already taken, and then clipped mid-word.
fn row_width(i: usize, rows: usize, g: usize, width: usize) -> usize {
    if i < rows {
        width.saturating_sub(text_indent(g))
    } else {
        width
    }
}

/// Push a composed line right by `n` blank columns, so the sprite gutter's
/// cells stay empty and a picture placed over them shares nothing with text.
fn indent(l: Line<'_>, n: usize) -> Line<'_> {
    if n == 0 {
        return l;
    }
    let mut spans = Vec::with_capacity(l.spans.len() + 1);
    spans.push(Span::raw(" ".repeat(n)));
    spans.extend(l.spans);
    Line::from(spans)
}

/// The column widths the moves table uses, in three tiers.
///
/// The order of sacrifice is what a turn is *not* decided on: accuracy goes
/// first (most Gen 1 moves are 100 %, and the exceptions are memorable), then
/// the type column shortens to three letters, then power goes, and the name
/// and PP are what is left — because a move you cannot afford to use is the
/// one fact that stops the turn dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveCols {
    pub name: usize,
    /// Type column; 0 for no type at all.
    pub ty: usize,
    /// Whether the `POW` column is drawn.
    pub power: bool,
    /// Whether the `ACC` column is drawn.
    pub accuracy: bool,
}

impl MoveCols {
    /// The header row this tier's columns line up under. It is a legend as
    /// well as a label, which is what makes it worth the row that
    /// `[ MOVES ]` was not.
    pub fn head<'a>(self) -> Line<'a> {
        let mut cells = vec![
            ("MOVE", self.name, panel::Align::Left),
            ("PP", 5, panel::Align::Right),
        ];
        if self.ty > 0 {
            cells.insert(1, ("TYPE", self.ty, panel::Align::Left));
        }
        if self.power {
            cells.push(("POW", 3, panel::Align::Right));
        }
        if self.accuracy {
            cells.push(("ACC", 3, panel::Align::Right));
        }
        panel::table_head(&cells)
    }
}

pub fn move_cols(width: usize) -> MoveCols {
    if width >= 34 {
        MoveCols {
            name: width - 24,
            ty: 8,
            power: true,
            accuracy: true,
        }
    } else if width >= 26 {
        MoveCols {
            name: width - 16,
            ty: 4,
            power: true,
            accuracy: false,
        }
    } else if width >= 18 {
        MoveCols {
            name: width - 6,
            ty: 0,
            power: false,
            accuracy: false,
        }
    } else {
        MoveCols {
            name: width,
            ty: 0,
            power: false,
            accuracy: false,
        }
    }
}

/// One row of the moves table.
///
/// **A zero-power move gets a dash, never a `0` and never the unknown glyph.**
/// A status move genuinely does no direct damage; printing `0` invites the
/// reader to compare it against a 40 as if they were the same kind of number,
/// and printing `?` claims we could not read it.
fn move_row<'a>(mv: &MoveSlot, c: MoveCols) -> Line<'a> {
    let out_of_pp = mv.pp == 0;
    let low_pp = !out_of_pp && mv.pp_max > 0 && mv.pp * 4 <= mv.pp_max;
    let mut spans = vec![Span::styled(
        panel::Align::Left.fit(&mv.name, c.name),
        if out_of_pp { dim() } else { panel::value() },
    )];
    if c.ty > 0 {
        spans.push(Span::raw(" "));
        let t = if mv.type_name.chars().count() > c.ty {
            mv.type_name.chars().take(c.ty).collect::<String>()
        } else {
            mv.type_name.clone()
        };
        spans.push(Span::styled(
            panel::Align::Left.fit(&t, c.ty),
            Style::default().fg(panel::type_color(&mv.type_name)),
        ));
    }
    spans.push(Span::styled(
        format!(" {:>2}/{:<2}", mv.pp, mv.pp_max),
        Style::default().fg(if out_of_pp {
            panel::ink::BAD
        } else if low_pp {
            panel::ink::WARN
        } else {
            panel::ink::VALUE
        }),
    ));
    if c.power {
        spans.push(Span::styled(
            format!(
                " {:>3}",
                if mv.power == 0 {
                    "—".to_string()
                } else {
                    mv.power.to_string()
                }
            ),
            if mv.power == 0 { dim() } else { panel::value() },
        ));
    }
    if c.accuracy {
        spans.push(Span::styled(
            format!(" {:>3}", mv.accuracy),
            if mv.accuracy >= 100 {
                dim()
            } else {
                Style::default().fg(panel::ink::WARN)
            },
        ));
    }
    Line::from(spans)
}

/// The detail view for one Pokémon: everything the game keeps to itself.
///
/// The heading names **whose** Pokémon this is. Leaving that to be inferred
/// from the picture is exactly the bug this replaced: during a battle the
/// sprite and the numbers can legitimately be different Pokémon, so the block
/// has to say which one it is describing.
///
/// `gutter` is the indent its own picture takes; the first [`FOCUS_ROWS`] rows
/// after the rule leave it blank.
pub fn detail_lines<'a>(
    m: &Mon,
    opponent: bool,
    mods: Option<terminalgb_plugin_trainer::derived::StatMods>,
    width: usize,
) -> Vec<Line<'a>> {
    detail_lines_indented(m, opponent, mods, width, 0, 0)
}

/// [`detail_lines`] with an explicit gutter indent for its picture.
pub fn detail_lines_indented<'a>(
    m: &Mon,
    opponent: bool,
    mods: Option<terminalgb_plugin_trainer::derived::StatMods>,
    width: usize,
    gutter: usize,
    gutter_rows: usize,
) -> Vec<Line<'a>> {
    let frac = m.hp_fraction();
    let hp = m.hidden_power();
    let text_w = width.saturating_sub(text_indent(gutter));
    let bar = 12.min(text_w.saturating_sub(14));
    // A wild Pokémon's "nickname" is its species name, and an opponent may
    // have none at all — an empty bold field followed by `(CHARMANDER)` reads
    // as a missing value rather than as an absent one.
    let (title, sub) = if m.nickname.is_empty() || m.nickname == m.name {
        (m.name.clone(), String::new())
    } else {
        (m.nickname.clone(), format!("({})", m.name))
    };
    let mut lines = vec![panel::rule(
        if opponent {
            "OPPONENT"
        } else {
            "YOUR POKéMON"
        },
        width,
    )];
    // Everything beside the picture; padded to its height by `indent_beside`.
    let mut beside: Vec<Line<'a>> = Vec::new();

    // --- the identity, beside the picture -----------------------------------
    let mut ident = vec![Span::styled(
        format!("{} ", clip(&title, text_w.saturating_sub(8))),
        Style::default()
            .fg(if opponent {
                panel::ink::FOE
            } else {
                panel::ink::VALUE
            })
            .add_modifier(Modifier::BOLD),
    )];
    ident.push(Span::styled(format!("L{}", m.level), panel::value()));
    beside.push(Line::from(ident));

    let mut second = vec![Span::styled(format!("#{:03} ", m.dex), dim())];
    second.extend(type_spans(m, text_w.saturating_sub(6)));
    if !sub.is_empty() {
        second.push(Span::styled(format!(" {sub}"), dim()));
    }
    beside.push(panel::clamp_line(Line::from(second), text_w));

    beside.push(Line::from(vec![
        Span::styled("HP ", panel::label()),
        panel::hp_bar_span(frac, bar),
        Span::styled(format!(" {}/{}", m.hp, m.max_hp), panel::value()),
    ]));

    let w = |i: usize| row_width(i, gutter_rows, gutter, width);
    beside.push(stat_line(
        m.attack,
        m.defense,
        m.speed,
        m.special,
        w(beside.len()),
    ));
    if let Some(l) = stat_mod_line(mods.or(m.stat_mods), "MOD", w(beside.len())) {
        beside.push(l);
    }
    indent_beside(&mut beside, gutter, gutter_rows);
    lines.append(&mut beside);

    // --- and the rest, full width, below the picture ------------------------
    let c = move_cols(width);
    lines.push(c.head());
    for i in 0..4 {
        match m.moves.get(i) {
            Some(mv) => lines.push(move_row(mv, c)),
            // An empty move slot is genuinely empty, not unknown — so it gets a
            // dash rather than the unknown glyph.
            None => lines.push(Line::from(Span::styled("—", dim()))),
        }
    }

    lines.push(panel::rule("HIDDEN", width));
    // **DVs and stat experience share one legend, so they share one set of
    // columns.** Five numbers under `hp atk def spd spc` is one glance;
    // `5/10/11/12/13` is counting on your fingers, and two rows of different
    // widths under a single header is worse than either — the legend would be
    // telling the truth about one of them and lying about the other.
    //
    // Stat experience runs to five digits, so five is the column width and the
    // DVs (0-15) sit in it with room to spare. The label column is what gives
    // when the panel is narrow, never a digit.
    let lab = if width >= 34 { 4 } else { 2 };
    lines.push(panel::table_head(&[
        ("", lab, panel::Align::Left),
        ("hp", 5, panel::Align::Right),
        ("atk", 5, panel::Align::Right),
        ("def", 5, panel::Align::Right),
        ("spd", 5, panel::Align::Right),
        ("spc", 5, panel::Align::Right),
    ]));
    lines.push(Line::from(vec![
        Span::styled(panel::Align::Left.fit("DV", lab), panel::label()),
        Span::styled(
            format!(
                " {:>5} {:>5} {:>5} {:>5} {:>5}",
                m.dvs.hp(),
                m.dvs.attack,
                m.dvs.defense,
                m.dvs.speed,
                m.dvs.special
            ),
            panel::value(),
        ),
    ]));
    // The running total is a nicety beside five numbers that are the point, so
    // it is appended only where it fits whole — a clipped `51/7` is worse than
    // no total at all.
    if let Some(last) = lines.last_mut() {
        let total = format!("  {}/75", m.dv_total());
        if line_width(last) + total.chars().count() <= width {
            last.spans.push(Span::styled(total, dim()));
        }
    }
    match &m.stat_exp {
        Some(e) => lines.push(Line::from(vec![
            Span::styled(panel::Align::Left.fit("EXP", lab), panel::label()),
            Span::styled(
                format!(
                    " {:>5} {:>5} {:>5} {:>5} {:>5}",
                    e.hp, e.attack, e.defense, e.speed, e.special
                ),
                panel::value(),
            ),
        ])),
        // The in-battle structure does not carry stat experience, so during a
        // battle this is unknown rather than zero.
        None => lines.push(Line::from(vec![
            Span::styled(panel::Align::Left.fit("EXP", lab), panel::label()),
            Span::styled(
                clip(
                    &format!(" {UNKNOWN} (not in the battle copy)"),
                    width.saturating_sub(lab),
                ),
                dim(),
            ),
        ])),
    }
    lines.push(Line::from(vec![
        Span::styled("HIDDEN POWER ", panel::label()),
        Span::styled(
            hp.type_name.to_string(),
            Style::default().fg(panel::type_color(hp.type_name)),
        ),
        Span::styled(format!(" {}", hp.power), panel::value()),
    ]));
    lines.push(Line::from(if m.would_be_shiny() {
        Span::styled(
            "★ would be SHINY in Gen 2",
            Style::default()
                .fg(panel::ink::SHINY)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("  not shiny if traded to Gen 2", dim())
    }));
    lines
}

/// The wild-encounter banner: one full-width row, drawn the moment a catchable
/// wild Pokémon appears and held for the whole encounter.
///
/// This is the loudest thing the frontend draws, and deliberately so — the
/// point of the feature is that the answer arrives *before* a Ball is spent,
/// which is no use at all if it is a dim line in a panel that is usually shut.
/// The verdict therefore gets its own row whether or not the trainer panel is
/// open (see [`crate::screen::tui`]).
///
/// # The wording is the feature
///
/// **Gen 1 has no shiny Pokémon.** Every string here says "would be shiny if
/// traded to Gen 2", never "is shiny", because the second one is false and
/// would send someone hunting for a colour change that Red/Blue cannot draw.
/// The long form spells the whole thing out; the short forms shrink the
/// *decoration*, never the qualifier.
pub fn encounter_banner(enc: &WildEncounter, width: usize) -> Line<'static> {
    let kind = if enc.kind == BattleType::Safari {
        "SAFARI"
    } else {
        "WILD"
    };
    let who = format!("{} L{}", clip(&enc.name, 11), enc.level);
    let dv = enc.dv_string();

    if enc.would_be_shiny() {
        let text = pick(
            width,
            &[
                format!(
                    " ★ {kind} {who} WOULD BE SHINY IF TRADED TO GEN 2 ★ \
                     DV {dv} (hp/atk/def/spd/spc) · nothing is shiny in Gen 1; \
                     this one turns shiny in Gold/Silver "
                ),
                format!(
                    " ★ {kind} {who} WOULD BE SHINY IF TRADED TO GEN 2 ★ \
                     DV {dv} · nothing is shiny in Gen 1 "
                ),
                format!(
                    " ★ {kind} {who} WOULD BE SHINY IF TRADED TO GEN 2 ★ \
                     DV {dv} "
                ),
                format!(" ★ {who} SHINY IF TRADED TO GEN 2 ★ DV {dv} "),
                format!(" ★ SHINY IF TRADED TO GEN 2 · DV {dv} "),
                " ★ SHINY IF TRADED TO GEN 2 ".to_string(),
                " ★ SHINY→GEN2 ".to_string(),
                "★→G2".to_string(),
                "★".to_string(),
            ],
        );
        // Pad to the full row: a highlighted strip right across the terminal is
        // the difference between "noticeable" and "impossible to miss", which
        // is the whole point of the feature.
        let pad = width.saturating_sub(text.chars().count());
        let text = format!("{text}{}", " ".repeat(pad));
        return Line::from(Span::styled(text, shiny_banner_style()));
    }

    // Not shiny. Still shown, and still with the numbers: a near miss is
    // interesting, and a banner that only ever appears for a shiny gives no way
    // to tell "working, and this one is ordinary" from "not working".
    let near = format!(
        "{}/3 tens, atk bit {}",
        enc.shiny_matches(),
        if enc.attack_bit_ok() { "yes" } else { "no" }
    );
    let text = pick(
        width,
        &[
            format!(
                " {kind} {who} · DV {dv} (hp/atk/def/spd/spc) · would NOT be \
                 shiny if traded to Gen 2 — needs def/spd/spc all 10 and bit 1 \
                 of the attack DV set ({near}) "
            ),
            format!(
                " {kind} {who} · DV {dv} · would NOT be shiny if traded to \
                 Gen 2 ({near}) "
            ),
            format!(" {kind} {who} · DV {dv} · not shiny if traded "),
            format!(" {who} · DV {dv} · not shiny "),
            format!(" DV {dv} · not shiny "),
        ],
    );
    Line::from(Span::styled(text, plain_banner_style()))
}

/// Black on bright yellow, bold: the one place in this UI that shouts.
pub fn shiny_banner_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::LightYellow)
        .add_modifier(Modifier::BOLD)
}

/// The ordinary-encounter row, which must be readable without competing with
/// the game.
pub fn plain_banner_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// The first candidate that fits `width`, or the last one truncated to it.
/// Candidates run longest to shortest.
fn pick(width: usize, candidates: &[String]) -> String {
    for c in candidates {
        if c.chars().count() <= width {
            return c.clone();
        }
    }
    let last = candidates.last().cloned().unwrap_or_default();
    last.chars().take(width).collect()
}

/// The in-battle stat stages, as a line that only exists when something is
/// actually modified.
///
/// This is the line that stops a `6` which is really a debuffed `10` from
/// reading like a Pokémon that was born weak — during a fight that distinction
/// is the whole job. It reports the **stage**, `-2` / `+1`, because that is
/// what a player reasons with and because it is the only way to say anything at
/// all about accuracy and evasion, neither of which has a value column.
/// Lowered is red, raised is green: direction is readable before the number is.
///
/// Absent outside a battle, and absent for a party copy — see
/// [`terminalgb_plugin_trainer::derived::StatMods`] for why stages must never be shown
/// from stored data.
pub fn stat_mod_line<'a>(
    mods: Option<terminalgb_plugin_trainer::derived::StatMods>,
    label: &str,
    width: usize,
) -> Option<Line<'a>> {
    let changed = mods?.changed();
    if changed.is_empty() {
        return None;
    }
    let head = format!("{label:<4}");
    let mut used = head.chars().count();
    let mut spans = vec![Span::styled(head, panel::label())];
    for (name, stage) in changed {
        let text = format!("{name}{stage:+}  ");
        if used + text.chars().count() > width {
            break;
        }
        used += text.chars().count();
        // Colour AND sign: `+2` is green, `-1` is red, and either is legible
        // with the colour thrown away. See `panel`'s redundancy rule.
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(panel::stage_color(stage))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Some(Line::from(spans))
}

/// The four battle stats, tabulated: labels dim and fixed-width, values in
/// columns that do not move as the numbers change.
///
/// Aligning them is most of what makes four numbers readable at a glance —
/// `ATK 42   DEF 8    SPD 61` reads as a table, `ATK 42 DEF 8 SPD 61` reads as
/// a sentence and has to be parsed.
fn stat_line<'a>(atk: u16, def: u16, spd: u16, spc: u16, width: usize) -> Line<'a> {
    let cells = [("ATK", atk), ("DEF", def), ("SPD", spd), ("SPC", spc)];
    // `ATK 999 ` x 4 = 32; the last cell needs no trailing pad.
    if width >= 31 {
        let mut spans = Vec::with_capacity(8);
        for (i, (l, v)) in cells.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(format!("{l} "), panel::label()));
            spans.push(Span::styled(format!("{v:<3}"), panel::value()));
        }
        return Line::from(spans);
    }
    // Too narrow for four labelled cells: **two** letters each, still in
    // columns. Not one — `SPD` and `SPC` both begin with `S`, so a
    // single-letter tier prints `S55 S29` and the reader cannot tell which
    // number is Speed. An ambiguous label is worse than a missing one, and
    // this tier is only two columns cheaper than the full one.
    let short = [("AT", atk), ("DF", def), ("SP", spd), ("SC", spc)];
    let mut spans = Vec::with_capacity(8);
    for (i, (l, v)) in short.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!("{l} "), panel::label()));
        spans.push(Span::styled(format!("{v:<3}"), panel::value()));
    }
    panel::clamp_line(Line::from(spans), width)
}

/// The opponent section — its own block, because a battle is the moment the
/// panel matters most.
///
/// It is drawn **above** the party (see [`body_lines`]) and gets the gutter's
/// first [`FOCUS_ROWS`] rows for its own picture. The party below it collapses
/// to one line each when the panel is short, so the team is still on screen.
/// Nothing about the opponent is behind a key.
pub fn opponent_lines<'a>(
    state: &TrainerState,
    focused: bool,
    width: usize,
) -> Vec<Line<'a>> {
    opponent_lines_indented(state, focused, width, 0, 0)
}

/// [`opponent_lines`] with an explicit gutter indent for its picture.
pub fn opponent_lines_indented<'a>(
    state: &TrainerState,
    focused: bool,
    width: usize,
    gutter: usize,
    gutter_rows: usize,
) -> Vec<Line<'a>> {
    let Some(o) = state.opponent.as_ref() else {
        return Vec::new();
    };
    let kind = match state.battle {
        // A Safari encounter and the old man's demonstration are both
        // `wIsInBattle == 1`; naming which is which is what stops the panel
        // implying a Ball can be spent when it cannot.
        BattleKind::Wild => match state.battle_type {
            BattleType::Safari => "safari".to_string(),
            BattleType::OldMan => "wild (demo)".to_string(),
            _ => "wild".to_string(),
        },
        BattleKind::Trainer => {
            if state.trainer_name.is_empty() {
                "trainer".to_string()
            } else {
                format!("trainer {}", state.trainer_name)
            }
        }
        BattleKind::None => return Vec::new(),
    };
    let text_w = width.saturating_sub(text_indent(gutter));
    let frac = o.hp_fraction();
    // Whose battle this is rides on the rule as its annotation, rather than
    // costing a row of its own under the name.
    let mut lines = vec![panel::rule_with("OPPONENT", Some(&kind), width)];
    // Everything that sits **beside** the picture. Laid out for `text_w`, and
    // padded to the picture's height by `indent_beside` so the table below can
    // never be half-indented.
    let mut beside: Vec<Line<'a>> = Vec::new();

    // --- the identity, beside the picture -----------------------------------
    let mut head = vec![
        Span::styled(if focused { "▸" } else { " " }, panel::label()),
        Span::styled(
            clip(&o.name, text_w.saturating_sub(6)),
            Style::default()
                .fg(panel::ink::FOE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" L{}", o.level), panel::value()),
    ];
    if let Some(s) = o.status.label() {
        head.push(Span::styled(
            format!(" {s}"),
            Style::default().fg(panel::status_color(s)),
        ));
    }
    beside.push(panel::clamp_line(Line::from(head), text_w));

    let mut second = vec![Span::styled(format!(" #{:03} ", o.dex), panel::label())];
    second.extend(type_spans(o, text_w.saturating_sub(7)));
    beside.push(panel::clamp_line(Line::from(second), text_w));

    let bar = 10.min(text_w.saturating_sub(10));
    beside.push(Line::from(vec![
        Span::raw(" "),
        panel::hp_bar_span(frac, bar),
        Span::styled(format!(" {}/{}", o.hp, o.max_hp), panel::value()),
    ]));

    // The stats follow the identity into the gutter's rows when the picture is
    // tall enough to want them, which is what a half-block picture is.
    let w = |i: usize| row_width(i, gutter_rows, gutter, width);
    beside.push(stat_line(
        o.attack,
        o.defense,
        o.speed,
        o.special,
        w(beside.len()),
    ));
    if let Some(l) = stat_mod_line(o.stat_mods, "MOD", w(beside.len())) {
        beside.push(l);
    }
    // The DVs of whatever is on the other side, always — they are the numbers
    // behind the encounter banner's verdict, and a trainer's Pokémon showing
    // the fixed `$98/$88` pair every single time is itself worth being able to
    // see.
    let dvs = format!(
        "{}/{}/{}/{}/{}",
        o.dvs.hp(),
        o.dvs.attack,
        o.dvs.defense,
        o.dvs.speed,
        o.dvs.special
    );
    let mut dv_row = vec![
        Span::styled("DV  ", panel::label()),
        Span::styled(dvs.clone(), panel::value()),
    ];
    // The legend is the part that goes when the panel is narrow — a truncated
    // `hp/atk/def/sp` is worse than none, and the numbers are the point. It is
    // measured against `text_w`, not `width`: this row is beside the picture,
    // so the gutter's columns are not its to spend.
    let legend = "  hp/atk/def/spd/spc";
    if 4 + dvs.chars().count() + legend.chars().count() <= w(beside.len()) {
        dv_row.push(Span::styled(legend, panel::label()));
    }
    beside.push(Line::from(dv_row));
    indent_beside(&mut beside, gutter, gutter_rows);
    lines.append(&mut beside);

    // --- and the rest, full width, below the picture -------------------------
    if state.battle == BattleKind::Wild && o.would_be_shiny() {
        lines.push(Line::from(Span::styled(
            "★ would be SHINY if traded to Gen 2",
            shiny_banner_style(),
        )));
    }

    // Moves, in the same table the detail view uses — an Ironmon run reads
    // these before it reads anything else on the panel, and reading them
    // against *the same columns in the same order* on both screens is what
    // makes the two views one design rather than two.
    if o.moves.is_empty() {
        // A freshly-encountered Pokémon whose moves have not loaded yet.
        lines.push(Line::from(Span::styled(format!("moves {UNKNOWN}"), dim())));
    } else {
        let c = move_cols(width);
        lines.push(c.head());
        for mv in &o.moves {
            lines.push(move_row(mv, c));
        }
    }
    // The roster line closes the section: a trainer battle is as much about
    // what is still in their belt as about what is on the field. When the strip
    // of pictures is drawn above, this is its caption; when it is not, this is
    // the whole report.
    lines.extend(catch_lines(state, width));
    if state.battle == BattleKind::Trainer {
        let l = team_line(state, width, true);
        if line_width(&l) > 0 {
            lines.push(l);
        }
    }
    // Your own Pokémon's stages, on the same block as the opponent's. A
    // Screech landing on you matters exactly as much as a Growl landing on
    // them, and reading it should not need a keypress.
    if let Some(l) = stat_mod_line(
        state.active.as_ref().and_then(|a| a.stat_mods),
        "YOU",
        width,
    ) {
        lines.push(l);
    }
    lines
}

/// The live catch chance, one line per encounter, compared across the balls a
/// player actually chooses between.
///
/// **Only for a wild encounter** — a trainer's Pokémon cannot be caught, and a
/// percentage there would be worse than nothing. The number recomputes on every
/// sample, so it moves as HP comes off and status lands, and a `▲` / `▼` says
/// which way it went since the last look, because "watch it go up" was the ask.
///
/// The arithmetic is Gen 1's own, disassembled from the cartridge — see
/// [`terminalgb_plugin_trainer::capture`], including why a Great Ball and an Ultra Ball
/// trade places depending on the target's HP.
pub fn catch_lines<'a>(state: &TrainerState, width: usize) -> Vec<Line<'a>> {
    let odds = state.catch_odds();
    if odds.is_empty() || !state.catchable() {
        return Vec::new();
    }
    let arrow = match state.catch_delta {
        Some(d) if d > 0.0005 => Some(("▲", Color::Green)),
        Some(d) if d < -0.0005 => Some(("▼", Color::Red)),
        _ => None,
    };
    // Widest first: every ball with a decimal, then every ball rounded, then
    // just the best one. The *comparison* is the decision a player is making,
    // so it is the last thing to go, not the first.
    let full: Vec<String> = odds
        .iter()
        .map(|o| format!("{} {:.1}%", o.ball.short(), o.chance * 100.0))
        .collect();
    let terse: Vec<String> = odds
        .iter()
        .map(|o| format!("{} {:.0}%", o.ball.short(), o.chance * 100.0))
        .collect();
    let best = odds
        .iter()
        .max_by(|a, b| a.chance.total_cmp(&b.chance))
        .expect("odds is non-empty");
    let tail = if arrow.is_some() { 2 } else { 0 };
    let body = if full.join("  ").chars().count() + tail <= width {
        full.join("  ")
    } else if terse.join("  ").chars().count() + tail <= width {
        terse.join("  ")
    } else {
        // Last resort: the number alone. `clamp_line` would truncate a `%` off
        // the end, which reads as a different number entirely.
        let bare = format!("{:.0}%", best.chance * 100.0);
        let labelled = format!("CATCH {bare}");
        if labelled.chars().count() + tail <= width {
            labelled
        } else {
            bare
        }
    };
    let mut spans = vec![Span::styled(
        body,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some((glyph, colour)) = arrow {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(glyph, Style::default().fg(colour)));
    }
    let mut lines = vec![Line::from(spans)];
    // The one thing worth spelling out: below a quarter of maximum HP the HP
    // term saturates and no further chip damage helps at all. A player who does
    // not know that keeps whittling for nothing.
    if best.hp_maxed && width >= 30 {
        lines.push(Line::from(Span::styled(
            "HP is as low as it counts — status next",
            dim(),
        )));
    }
    lines
}

/// The opposing trainer's roster as one line of markers, for when the strip of
/// pictures does not fit — or as the caption under it when it does.
///
/// Reads as `TEAM ● ● ○ ?  2 left`: a filled marker is standing, a hollow one
/// has fainted, `?` is a slot that did not read back. **Fainted and unknown are
/// never the same glyph** — "beaten" and "not yet legible" are opposite facts
/// and confusing them is how a tracker gets someone killed.
pub fn team_line<'a>(state: &TrainerState, width: usize, label: bool) -> Line<'a> {
    if state.battle != BattleKind::Trainer {
        return Line::from(Span::raw(""));
    }
    if state.enemy_team.is_empty() {
        let why = if state.enemy_team_unknown {
            format!("TEAM {UNKNOWN}")
        } else {
            String::new()
        };
        return Line::from(Span::styled(clip(&why, width), dim()));
    }
    let standing = state
        .enemy_team
        .iter()
        .filter(|m| m.known && !m.is_fainted())
        .count();
    let mut spans = Vec::new();
    let mut used = 0usize;
    if label {
        spans.push(Span::styled("TEAM ", dim()));
        used += 5;
    }
    for (i, m) in state.enemy_team.iter().enumerate() {
        if used + 2 > width {
            break;
        }
        if i > 0 {
            spans.push(Span::raw(" "));
            used += 1;
        }
        let (glyph, style) = if !m.known {
            (UNKNOWN, dim())
        } else if m.is_fainted() {
            ("○", dim())
        } else {
            ("●", Style::default().fg(Color::Red))
        };
        spans.push(Span::styled(glyph, style));
        used += 1;
    }
    let tail = format!("  {standing} left");
    if used + tail.chars().count() <= width {
        spans.push(Span::styled(tail, dim()));
    }
    Line::from(spans)
}

/// The names of the roster, so "what is coming next" is answerable without the
/// pictures. Two per row while they fit.
pub fn team_names<'a>(state: &TrainerState, width: usize) -> Vec<Line<'a>> {
    if state.enemy_team.len() < 2 {
        return Vec::new();
    }
    let per = if width >= 30 { 2 } else { 1 };
    let cell = if per == 2 { (width - 1) / 2 } else { width };
    let label = |m: &TeamMon| -> String {
        if !m.known {
            UNKNOWN.to_string()
        } else {
            format!("{} {}", pad(&m.name, cell.saturating_sub(4)), m.level)
        }
    };
    state
        .enemy_team
        .chunks(per)
        .map(|chunk| {
            let mut spans = Vec::new();
            for (i, m) in chunk.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(
                    pad(&label(m), cell),
                    if m.is_fainted() || !m.known {
                        dim()
                    } else {
                        Style::default().fg(Color::Reset)
                    },
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// The composed body: the lines, and **where the pictures go in them**.
///
/// The gutters are reported as row offsets rather than rectangles because
/// only the composer knows how many rows precede each block, and only the
/// layout knows where the body landed on screen. Keeping them apart is what
/// lets the whole body be unit-tested without a terminal
/// ([`the_party_gutter_lines_up_with_the_party_rows`] is that test) and what
/// guarantees the two can never disagree about which rows a picture covers.
pub struct Body<'a> {
    pub lines: Vec<Line<'a>>,
    /// Row within [`lines`](Body::lines) where the focused block's picture
    /// starts, and how many rows it spans.
    pub focus_gutter: Option<(u16, u16)>,
    /// Row within [`lines`](Body::lines) where the party gutter starts, and
    /// how many slots it covers. Each slot spans [`SLOT_ROWS`].
    pub party_gutter: Option<(u16, u16)>,
    /// Columns the composer indented its rows by. Zero when it laid out
    /// without pictures.
    pub gutter: u16,
    /// What the composer was told this terminal can draw.
    pub pictures: Pictures,
}

impl Body<'_> {
    pub fn rows(&self) -> u16 {
        self.lines.len() as u16
    }

    /// Whether this composition actually reserved gutter columns.
    pub fn wants_gutter(&self) -> bool {
        self.gutter > 0
    }
}

/// Compose the body, choosing the tier that fits the rows available.
///
/// **The order of sacrifice, and it is the maintainer's requirement made
/// mechanical:** the six party slots are never the thing that goes. What goes,
/// in order, is
///
/// 1. the party's *second* line per slot — species, HP numbers and types move
///    into the one headline row's own columns;
/// 2. with it, the party gutter, because a one-row sprite is a smudge;
/// 3. and only then does anything fall off the bottom, which the frontend's
///    `centre_in` makes the *last* rows of the party rather than the header.
///
/// Note what is **not** in that list: the pictures never cost a row, so they
/// are never traded against the text. A party cell spans exactly the two rows
/// its own text needed. That is the whole reason six small pictures are
/// affordable where the single large one they replace was not.
pub fn compose_body<'a>(
    state: &TrainerState,
    focus: Option<Focus>,
    detail: bool,
    width: usize,
    pictures: Pictures,
    rows_available: u16,
) -> Body<'a> {
    let gutter = pictures.gutter(width as u16);
    let kind = if gutter == 0 {
        Pictures::None
    } else {
        pictures
    };
    let full = body_rows(state, focus, detail, width, gutter, kind, false);
    if full.rows() <= rows_available || rows_available == 0 {
        return full;
    }
    // Doesn't fit: collapse the party to one line each. The party gutter goes
    // with it — see the doc comment. The focused block keeps its picture,
    // because its rows did not change.
    let compact = body_rows(state, focus, detail, width, gutter, kind, true);
    if compact.rows() < full.rows() {
        compact
    } else {
        full
    }
}

/// Everything below the header, assembled in the order the situation calls for.
///
/// **Out of battle** the party is the panel. **In battle** the opponent goes
/// first — beside its own picture — and the party follows it, so the team is
/// still on screen whatever else is happening. **In the detail view** the
/// focused Pokémon's full page goes first and *the party is still under it*,
/// which is the single biggest change here: `d` used to replace the team with
/// one Pokémon's numbers, so the mode that exists to compare made the thing
/// being compared against invisible.
pub fn body_lines<'a>(
    state: &TrainerState,
    focus: Option<Focus>,
    detail: bool,
    width: usize,
) -> Vec<Line<'a>> {
    body_rows(state, focus, detail, width, 0, Pictures::None, false).lines
}

#[allow(clippy::too_many_arguments)]
fn body_rows<'a>(
    state: &TrainerState,
    focus: Option<Focus>,
    detail: bool,
    width: usize,
    gutter: u16,
    pictures: Pictures,
    compact: bool,
) -> Body<'a> {
    let g = gutter as usize;
    let focus_rows = pictures.focus_rows();
    let mut lines: Vec<Line<'a>> = Vec::new();
    let mut focus_gutter = None;
    let fighting = state.opponent.is_some() && state.battle != BattleKind::None;
    let selected = match focus {
        Some(Focus::Party(i)) => Some(i),
        _ => None,
    };

    if detail {
        let Some((f, m)) = focus.and_then(|f| focused_mon(state, f).map(|m| (f, m)))
        else {
            return Body {
                lines: vec![Line::from(Span::styled("  no Pokémon yet", dim()))],
                focus_gutter: None,
                party_gutter: None,
                gutter: 0,
                pictures: Pictures::None,
            };
        };
        // The party copy carries no stages — by design, they do not survive a
        // switch. So when the focused party member IS the one in the fight, the
        // stages come from the battle copy beside it.
        let mods = match f {
            Focus::Opponent => state.opponent.as_ref().and_then(|o| o.stat_mods),
            Focus::Party(i) if active_slot(state) == Some(i) => {
                state.active.as_ref().and_then(|a| a.stat_mods)
            }
            Focus::Party(_) => None,
        };
        // Row 0 is the section rule; the picture starts on the row after it.
        focus_gutter = (g > 0 && focus_rows > 0).then_some((1u16, focus_rows));
        lines.extend(detail_lines_indented(
            m,
            f.is_opponent(),
            mods,
            width,
            g,
            focus_rows as usize,
        ));
        // The roster follows the opponent's own numbers even here, because
        // "what else has this trainer got" is part of the same decision.
        if f.is_opponent() && state.battle == BattleKind::Trainer {
            let l = team_line(state, width, true);
            if line_width(&l) > 0 {
                lines.push(l);
                lines.extend(team_names(state, width));
            }
        }
    } else if fighting {
        focus_gutter = (g > 0 && focus_rows > 0).then_some((1u16, focus_rows));
        lines.extend(opponent_lines_indented(
            state,
            focus == Some(Focus::Opponent),
            width,
            g,
            focus_rows as usize,
        ));
    }

    // The party, in both modes and always last, so its rows are the ones a
    // short panel clips and its gutter can never be pushed off by a block
    // above it changing length.
    let party_pictures = pictures.party() && !compact && !state.party.is_empty();
    let note =
        (!pictures.party() && pictures != Pictures::None).then_some(NO_PARTY_PICTURES);
    lines.push(panel::rule_with(
        "PARTY",
        Some(&format!("{}/6", state.party.len())),
        width,
    ));
    let party_at = lines.len() as u16;
    let rows_before = lines.len();
    let party_g = if party_pictures { g } else { 0 };
    lines.extend(party_lines_indented(
        state, selected, width, compact, party_g,
    ));
    let party_gutter =
        party_pictures.then(|| (party_at, ((lines.len() - rows_before) / 2) as u16));
    // **Say what was dropped, once, under the thing that is missing.** A
    // silent absence reads as a defect in the emulator; six unrecognisable
    // smudges would be worse than either.
    if let Some(n) = note {
        lines.push(Line::from(Span::styled(clip(n, width), dim())));
    }

    Body {
        lines: lines
            .into_iter()
            .map(|l| panel::clamp_line(l, width))
            .collect(),
        focus_gutter,
        party_gutter,
        gutter: if g > 0 { gutter } else { 0 },
        pictures,
    }
}

/// Which party slot the battle engine currently has out, by matching the
/// in-battle copy back to the party. Gen 1 keeps no index we can read directly;
/// nickname *and* species together are unambiguous in practice, and a miss just
/// means no slot is underlined.
pub fn active_slot(state: &TrainerState) -> Option<usize> {
    let a = state.active.as_ref()?;
    // Nickname *and* species is unambiguous when both are readable. But
    // `wBattleMonNick` is filled a little after `wBattleMon` and can be empty
    // or stale for the first frames of a battle, and a marker that appears a
    // beat late reads as a bug — so fall back to species and level, then to
    // species alone. A wrong guess here costs an underline, not a number.
    state
        .party
        .iter()
        .position(|m| m.species == a.species && m.nickname == a.nickname)
        .or_else(|| {
            state
                .party
                .iter()
                .position(|m| m.species == a.species && m.level == a.level)
        })
        .or_else(|| state.party.iter().position(|m| m.species == a.species))
}

/// A type label that fits `max` columns: the full `GRASS/POISON` when it fits,
/// otherwise three-letter abbreviations, which stay readable where a truncation
/// (`GRASS/POI…`) does not.
/// Pad or clip to exactly `n` columns, so a column stays a column.
fn pad(s: &str, n: usize) -> String {
    let t = clip(s, n);
    let w = t.chars().count();
    format!("{t}{}", " ".repeat(n.saturating_sub(w)))
}

/// Clip to at most `n` columns, marking that something was cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    if n == 0 {
        return String::new();
    }
    s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
}

fn thousands(v: u32) -> String {
    let s = v.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Render a paragraph's worth of lines to plain strings, for tests and for the
/// `--dump` layout preview in `examples/trainer_layout.rs`.
pub fn plain(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

/// The width of a rendered line in terminal columns. Every glyph the panel uses
/// is single-width, so counting chars is counting columns — which the
/// `every_glyph_the_panel_uses_is_single_width` test pins.
pub fn line_width(l: &Line<'_>) -> usize {
    l.spans.iter().map(|s| s.content.chars().count()).sum()
}

/// The whole panel as a `Paragraph`, used by the message screens.
pub fn message<'a>(text: &'a str, title: &'a str) -> Paragraph<'a> {
    Paragraph::new(text)
        .wrap(ratatui::widgets::Wrap { trim: true })
        .style(dim())
        .block(panel_block(title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_plugin_trainer::gen1::{Dvs, MoveSlot, Status};
    use terminalgb_plugin_trainer::TeamMon;

    fn mon(hp: u16, max: u16) -> Mon {
        Mon {
            species: 0x99,
            dex: 1,
            name: "BULBASAUR".into(),
            nickname: "BULBY".into(),
            level: 15,
            hp,
            max_hp: max,
            status: Status(0),
            types: ("GRASS".into(), "POISON".into()),
            attack: 25,
            defense: 26,
            speed: 24,
            special: 30,
            moves: Vec::new(),
            dvs: Dvs::from_bytes(0x9A, 0x88),
            stat_exp: None,
            stat_mods: None,
        }
    }

    fn filled_state() -> TrainerState {
        let mut st = TrainerState {
            in_game: true,
            player_name: "BLUE".into(),
            player_id: 62800,
            money: 3_000,
            play_time: (0, 1, 41),
            badges: 0b0000_0011,
            ..Default::default()
        };
        for i in 0..3 {
            let mut m = mon(20 + i as u16, 41);
            m.nickname = format!("VERYLONGNAME{i}");
            m.moves = vec![
                MoveSlot {
                    id: 1,
                    name: "THUNDERBOLT".into(),
                    pp: 15,
                    pp_max: 15,
                    pp_ups: 0,
                    power: 95,
                    type_name: "ELECTRIC".into(),
                    accuracy: 100,
                },
                // Zero power and a sub-100 accuracy, so the two columns that
                // exist to distinguish moves are actually distinguishing.
                MoveSlot {
                    id: 2,
                    name: "QUICK ATTACK".into(),
                    pp: 0,
                    pp_max: 30,
                    pp_ups: 0,
                    power: 0,
                    type_name: "NORMAL".into(),
                    accuracy: 85,
                },
            ];
            st.party.push(m);
        }
        st
    }

    fn battling() -> TrainerState {
        let mut st = filled_state();
        st.battle = BattleKind::Trainer;
        st.trainer_name = "JOEY".into();
        let mut o = mon(18, 20);
        o.name = "CHARMANDER".into();
        // One Growl and a Screech landed: exactly the case the panel exists to
        // make legible.
        o.stat_mods = Some(terminalgb_plugin_trainer::derived::StatMods::from_bytes([
            6, 7, 7, 7, 7, 7,
        ]));
        o.types = ("FIRE".into(), "FIRE".into());
        o.moves = vec![
            MoveSlot {
                id: 3,
                name: "SCRATCH".into(),
                pp: 35,
                pp_max: 35,
                pp_ups: 0,
                power: 40,
                type_name: "NORMAL".into(),
                accuracy: 100,
            },
            MoveSlot {
                id: 4,
                name: "EMBER".into(),
                pp: 25,
                pp_max: 25,
                pp_ups: 0,
                power: 40,
                type_name: "FIRE".into(),
                accuracy: 100,
            },
        ];
        st.opponent = Some(o);
        let mut active = st.party[1].clone();
        active.stat_mods =
            Some(terminalgb_plugin_trainer::derived::StatMods::from_bytes([
                7, 5, 7, 7, 7, 7,
            ]));
        st.active = Some(active);
        st
    }

    #[test]
    fn the_panel_pieces_never_overlap_at_any_size() {
        for w in [10u16, 20, 40, 80, 200] {
            for h in [1u16, 3, 5, 6, 12, 13, 40, 100] {
                let area = Rect::new(3, 2, w, h);
                let l = layout(area, Pictures::Image);
                for a in [l.header, l.body] {
                    assert!(
                        a.x >= area.x
                            && a.y >= area.y
                            && a.right() <= area.right()
                            && a.bottom() <= area.bottom(),
                        "{a:?} escapes {area:?}"
                    );
                }
                assert!(
                    l.header.bottom() <= l.body.y,
                    "header runs into the body at {w}x{h}"
                );
                // Every gutter a picture could be asked for stays inside the
                // body and inside the reserved columns.
                for row in 0..l.body.height {
                    for rows in 1..=FOCUS_ROWS.max(SLOT_ROWS * 6) {
                        if let Some(g) = l.gutter_rect(row, rows) {
                            assert!(
                                g.x >= l.body.x
                                    && g.right() <= l.body.x + l.gutter
                                    && g.y >= l.body.y
                                    && g.bottom() <= l.body.bottom(),
                                "gutter {g:?} escapes body {:?} at {w}x{h}",
                                l.body
                            );
                        }
                    }
                }
            }
        }
    }

    /// **The pictures cost columns, never rows.** This is the property the
    /// whole redesign turns on: the panel that shows six party pictures is
    /// exactly as tall as the panel that shows none, so "always show all six"
    /// costs nothing vertically. The single large sprite it replaces took
    /// eleven rows of a twenty-four-row panel.
    #[test]
    fn pictures_cost_columns_and_never_rows() {
        let st = filled_state();
        for w in [28usize, 34, 40, 60] {
            let with =
                compose_body(&st, Some(Focus::Party(0)), false, w, Pictures::Image, 60);
            let without =
                compose_body(&st, Some(Focus::Party(0)), false, w, Pictures::None, 60);
            assert_eq!(
                with.rows(),
                without.rows(),
                "{w} columns: a gutter changed the row count"
            );
            assert!(with.gutter > 0, "{w} columns should afford a gutter");
            assert_eq!(without.gutter, 0);
        }
        // …and the panel that results is the same height either way.
        let area = Rect::new(0, 0, 40, 40);
        let a = compose_body(&st, None, false, 38, Pictures::Image, 60);
        let b = compose_body(&st, None, false, 38, Pictures::None, 60);
        assert_eq!(
            layout_with_pictures(area, &a)
                .centre_in(area, a.rows())
                .frame
                .height,
            layout_with_pictures(area, &b)
                .centre_in(area, b.rows())
                .frame
                .height,
        );
    }

    /// The gutter a picture is placed over must cover **exactly** the rows of
    /// the text it belongs to. This is the invariant that keeps a sprite level
    /// with its own party row rather than one slot out — which is the failure
    /// a screenshot shows and a row count does not.
    #[test]
    fn the_party_gutter_lines_up_with_the_party_rows() {
        let st = filled_state();
        let body =
            compose_body(&st, Some(Focus::Party(0)), false, 38, Pictures::Image, 60);
        let (row, slots) = body.party_gutter.expect("a party gutter");
        assert_eq!(slots as usize, st.party.len(), "one cell per Pokémon");
        // The row the gutter starts on is the first party row, i.e. the one
        // after the `PARTY` rule — and the text on it is that Pokémon's.
        let text = plain(&body.lines);
        // The nickname is legitimately clipped in the row, so match its stem.
        let stem = |s: &str| s.chars().take(6).collect::<String>();
        assert!(
            text[row as usize].contains(&stem(&st.party[0].nickname))
                && text[row as usize].contains("▸1"),
            "gutter starts on {row}, which reads {:?}",
            text[row as usize]
        );
        // …and the last row it covers is the last party row.
        let last = (row + slots * SLOT_ROWS - 1) as usize;
        assert_eq!(last, body.lines.len() - 1, "gutter overruns the party");
        assert!(
            text[last].contains(&stem(&st.party[2].name)),
            "{:?}",
            text[last]
        );
    }

    /// The focused block's picture likewise sits on its own three rows, under
    /// the section rule and never over it.
    #[test]
    fn the_focus_gutter_sits_under_its_own_rule() {
        let st = battling();
        for detail in [false, true] {
            let body =
                compose_body(&st, Some(Focus::Opponent), detail, 38, Pictures::Image, 80);
            let (row, rows) = body.focus_gutter.expect("a focus gutter");
            assert_eq!(rows, FOCUS_ROWS);
            let text = plain(&body.lines);
            assert!(
                text[(row - 1) as usize].contains("OPPONENT"),
                "the rule is not directly above the picture: {:?}",
                text[(row - 1) as usize]
            );
            // The fixture's opponent carries a nickname, so the identity row
            // is the *displayed title* — which is the point: the picture and
            // the name beside it must be the same Pokémon.
            let ident = &text[row as usize];
            assert!(
                ident.contains("CHARMANDER")
                    || ident.contains(&st.opponent.as_ref().unwrap().nickname),
                "the picture's first row is not the identity: {ident:?}"
            );
        }
    }

    /// **A terminal with no image protocol still gets a picture, and is told
    /// what it is not getting.**
    ///
    /// This is the reproduction of the fault the redesign found: `sprite_image`
    /// is `None` for anything but Kitty and iTerm2, so on a VTE-family
    /// terminal — the maintainer's own, and the one Fedora builds without sixel —
    /// the trainer panel drew no Pokémon at all and said nothing about it.
    ///
    /// The two halves of the answer are both asserted, because either alone is
    /// wrong: half-blocks **do** get the focused picture, at more rows because
    /// a cell there is two samples rather than sixteen pixels; and they **do
    /// not** get six party pictures, because a party slot is two rows, four
    /// samples, and four samples is not a Pokémon — so the panel says so
    /// instead of drawing six smudges.
    #[test]
    fn a_terminal_with_no_image_protocol_still_gets_a_picture_and_is_told_why() {
        let st = battling();
        let blocks = compose_body(
            &st,
            Some(Focus::Opponent),
            false,
            38,
            Pictures::Halfblocks,
            80,
        );
        let (_, rows) = blocks
            .focus_gutter
            .expect("half-blocks must still draw the focused Pokémon");
        assert!(
            rows > FOCUS_ROWS,
            "a half-block cell is two samples, so it needs more rows than an \
             image one: got {rows}, an image picture gets {FOCUS_ROWS}"
        );
        assert_eq!(blocks.party_gutter, None, "four samples is not a Pokémon");
        let text = plain(&blocks.lines).join("\n");
        assert!(
            text.contains(NO_PARTY_PICTURES),
            "the panel must say what it is not drawing:\n{text}"
        );
        // …and it says it once, not once per slot.
        assert_eq!(text.matches(NO_PARTY_PICTURES).count(), 1);

        // An image-protocol terminal gets the party pictures and no notice —
        // a line explaining an absence that is not happening is noise.
        let image =
            compose_body(&st, Some(Focus::Opponent), false, 38, Pictures::Image, 80);
        assert!(image.party_gutter.is_some());
        assert!(!plain(&image.lines).join("\n").contains(NO_PARTY_PICTURES));
    }

    /// **A picture never overlaps a table's header.** The half-block picture is
    /// twice as tall as the image one, so it reaches past the identity rows —
    /// and a moves header indented over rows that are not is a legend that
    /// lies about its own columns. Caught in a screenshot; pinned here.
    #[test]
    fn a_tall_picture_never_half_indents_the_table_under_it() {
        let st = battling();
        for pictures in [Pictures::Halfblocks, Pictures::Image] {
            for detail in [false, true] {
                let body =
                    compose_body(&st, Some(Focus::Opponent), detail, 44, pictures, 90);
                let Some((row, rows)) = body.focus_gutter else {
                    continue;
                };
                let text = plain(&body.lines);
                let indent = text_indent(body.gutter as usize);
                // Every row the picture covers is blank in the gutter…
                for i in row..row + rows {
                    let l = &text[i as usize];
                    assert!(
                        l.chars().take(indent).all(|c| c == ' '),
                        "{pictures:?} detail={detail}: row {i} writes into the \
                         gutter: {l:?}"
                    );
                }
                // …and the moves table starts *below* it, whole.
                let head = text
                    .iter()
                    .position(|l| l.trim_start().starts_with("MOVE "))
                    .expect("a moves table");
                assert!(
                    head as u16 >= row + rows,
                    "{pictures:?} detail={detail}: the moves header at {head} is \
                     inside the picture's rows {row}..{}",
                    row + rows
                );
                assert!(
                    !text[head].starts_with(' '),
                    "{pictures:?} detail={detail}: the header is indented and its \
                     rows are not: {:?}",
                    text[head]
                );
            }
        }
    }

    /// A short panel gives up the party's second line — and the gutter with
    /// it, because a one-row sprite is a smudge — before it gives up a slot.
    #[test]
    fn a_short_panel_collapses_the_party_before_it_loses_one() {
        let st = filled_state();
        let full = compose_body(&st, None, false, 38, Pictures::Image, 60);
        assert!(full.wants_gutter());
        let squeezed =
            compose_body(&st, None, false, 38, Pictures::Image, full.rows() - 1);
        assert!(squeezed.rows() < full.rows());
        assert_eq!(
            squeezed.party_gutter, None,
            "a one-row sprite is a smudge and must not be drawn"
        );
        // Every Pokémon is still named.
        let text = plain(&squeezed.lines).join("\n");
        for (i, _m) in st.party.iter().enumerate() {
            // Names are clipped at this width; the slot number is not, and it
            // is what says the slot is still on the panel.
            assert!(
                plain(&squeezed.lines).iter().any(|l| {
                    l.trim_start_matches([' ', '\u{25b8}', '\u{258c}'])
                        .starts_with(&format!("{} ", i + 1))
                }),
                "party slot {} was dropped:\n{text}",
                i + 1
            );
        }
    }

    /// …and so do the party columns: above the design width they grow rather
    /// than stranding the space, which is the only way text scales on a grid.
    #[test]
    fn the_party_columns_grow_with_a_wide_panel() {
        let narrow = party_cols(33);
        let wide = party_cols(60);
        assert!(wide.nick > narrow.nick && wide.bar > narrow.bar);
        // …but not without bound: past a point a longer bar is just a longer
        // bar, and the columns stay in proportion to one another.
        let huge = party_cols(200);
        assert_eq!((huge.nick, huge.bar, huge.ty), (18, 20, 16));
    }

    /// The design width must land in the *top* fixed tier once the gutter has
    /// taken its columns — a threshold set against the panel rather than
    /// against the text left of it silently demotes every default panel.
    #[test]
    fn the_design_width_keeps_the_widest_fixed_party_tier() {
        let inner = PANEL_W - 2;
        let text = inner - gutter_w(inner) - 1;
        assert_eq!(party_cols(text as usize), party_cols(32));
    }

    #[test]
    fn asking_for_no_pictures_leaves_no_gutter() {
        let l = layout(Rect::new(0, 0, PANEL_W, 40), Pictures::None);
        assert_eq!(l.gutter, 0);
        assert_eq!(l.gutter_rect(0, 2), None);
        assert_eq!(l.header.bottom(), l.body.y);
    }

    /// The whole point of the tiers: **nothing may overflow the panel**, at any
    /// width, in any of the three situations the body can be in. This is the
    /// guarantee the layout rests on — the arithmetic in [`party_cols`] is only
    /// as good as this test.
    #[test]
    fn no_line_overflows_at_any_width() {
        for w in 8usize..=60 {
            for (what, st) in [
                ("idle", filled_state()),
                ("battle", battling()),
                ("battle+team", with_team()),
            ] {
                for detail in [false, true] {
                    let mut lines = header_lines(&st, "Pokémon Blue", w);
                    for f in focus_ring(&st) {
                        lines.extend(body_lines(&st, Some(f), detail, w));
                    }
                    for l in &lines {
                        assert!(
                            line_width(l) <= w,
                            "{what}/detail={detail} at width {w}: {:?} is {} wide",
                            plain(std::slice::from_ref(l))[0],
                            line_width(l)
                        );
                    }
                }
            }
        }
    }

    /// Alignment discipline: the level, the HP bar and the HP numbers must land
    /// on the *same* column for every party member, whatever the values are.
    #[test]
    fn party_columns_line_up_however_the_values_change() {
        let mut st = filled_state();
        st.party[0].level = 5;
        st.party[0].hp = 7;
        st.party[0].nickname = "A".into();
        st.party[1].level = 100;
        st.party[1].hp = 999;
        st.party[1].max_hp = 999;
        st.party[1].nickname = "ABCDEFGHIJKLMNOP".into();
        for w in [40usize, 38, 34, 30, 24] {
            let lines = party_lines(&st, Some(0), w, false);
            let text = plain(&lines);
            let heads: Vec<&String> = text.iter().step_by(2).collect();
            let level_at: Vec<usize> = heads
                .iter()
                .map(|s| s.find(" L").expect("a level column"))
                .collect();
            assert!(
                level_at.windows(2).all(|p| p[0] == p[1]),
                "the level column moves at width {w}: {level_at:?}"
            );
            let bodies: Vec<&String> = text.iter().skip(1).step_by(2).collect();
            let slash: Vec<usize> = bodies
                .iter()
                .map(|s| s.find('/').expect("an HP column"))
                .collect();
            assert!(
                slash.windows(2).all(|p| p[0] == p[1]),
                "the HP column moves at width {w}: {slash:?}"
            );
        }
    }

    /// A battle must not push the party off the panel. The body has to fit the
    /// rows a 40x30 terminal leaves it — which is the smallest window the
    /// maintainer plays in — with every party member still named.
    #[test]
    fn a_battle_still_shows_the_whole_party() {
        let mut st = battling();
        while st.party.len() < 6 {
            st.party.push(mon(30, 41));
        }
        let panel = Rect::new(0, 0, PANEL_W, 30);
        let width = panel.width.saturating_sub(2) as usize;
        let rows = panel.height - 2 - HEADER_ROWS;
        let body = compose_body(
            &st,
            Some(Focus::Opponent),
            false,
            width,
            Pictures::Image,
            rows,
        );
        let l = layout_with_pictures(panel, &body);
        assert!(
            body.rows() <= l.body.height,
            "a battle overflows the body at 40x30: {} lines into {} rows",
            body.rows(),
            l.body.height
        );
        let text = plain(&body.lines).join("\n");
        assert!(
            text.contains("OPPONENT"),
            "the opponent has its own section"
        );
        assert!(text.contains("CHARMANDER"));
        assert!(text.contains("SCRATCH"), "and its moves are on screen");
        // Every party member is still listed, by slot number — the nicknames
        // are legitimately clipped at this width.
        for i in 0..st.party.len() {
            // By slot number, not by name: nicknames are legitimately clipped
            // at this width and the fixture repeats them.
            assert!(
                plain(&body.lines).iter().any(|l| {
                    l.trim_start_matches([' ', '\u{25b8}', '\u{258c}'])
                        .starts_with(&format!("{} ", i + 1))
                }),
                "party slot {} is not on the panel:\n{text}",
                i + 1
            );
        }
    }

    #[test]
    fn the_opponent_section_is_above_the_party() {
        let st = battling();
        let text = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        let o = text.find("OPPONENT").expect("an opponent section");
        let p = text.find("PARTY").expect("a party section");
        assert!(o < p, "the opponent must come first in a battle");
    }

    /// **Every section label starts in the same column**, which is the whole
    /// argument for a left-anchored rule over a centred one: the page can be
    /// scanned down its left edge instead of the eye travelling to the middle
    /// of each rule to find out where it is.
    ///
    /// The header has no rule at all — `[ TRAINER ]` and `[ BADGES ]` were two
    /// rows spent restating what the border's own title and the eight markers
    /// already say.
    #[test]
    fn every_section_label_starts_in_the_same_column() {
        let st = battling();
        let head = plain(&header_lines(&st, "Pokémon Blue", 38));
        assert_eq!(head.len(), HEADER_ROWS as usize);
        for l in &head {
            assert!(!l.contains('['), "the header spends no row on a rule: {l}");
        }
        let mut labelled = 0;
        for detail in [false, true] {
            for l in plain(&body_lines(&st, Some(Focus::Opponent), detail, 38)) {
                if let Some(at) = l.find("─ ") {
                    assert_eq!(at, 0, "a rule that does not start at column 0: {l}");
                    labelled += 1;
                }
            }
        }
        assert!(labelled >= 4, "expected OPPONENT/PARTY/MOVES-ish rules");
        let detail = plain(&body_lines(&st, Some(Focus::Party(0)), true, 38)).join("\n");
        assert!(detail.contains("─ YOUR POKéMON"));
        assert!(detail.contains("─ HIDDEN"));
        // `[ MOVES ]` became a column header, which is a legend as well as a
        // label — the row it costs now carries data.
        assert!(detail.contains("MOVE"), "{detail}");
        assert!(detail.contains("POW"), "{detail}");
        assert!(detail.contains("ACC"), "{detail}");
    }

    #[test]
    fn a_rule_is_left_anchored_and_exactly_the_width_asked_for() {
        // A rule always fills exactly the width it is given, down to the
        // point where the label itself no longer fits and gets clipped.
        for w in 5usize..=60 {
            assert_eq!(line_width(&rule("PARTY", w)), w, "at width {w}");
        }
        for w in 0usize..5 {
            assert!(line_width(&rule("PARTY", w)) <= w, "at width {w}");
        }
        assert_eq!(plain(&[rule("PARTY", 21)])[0], "─ PARTY ─────────────");
        assert_eq!(plain(&[rule("PARTY", 5)])[0], "PARTY");
    }

    #[test]
    fn the_badge_row_distinguishes_held_from_missing_two_ways() {
        let st = TrainerState {
            badges: 0b0000_0101,
            ..Default::default()
        };
        let line = badge_row(&st, 38);
        let text = plain(std::slice::from_ref(&line))[0].clone();
        assert_eq!(text.matches('●').count(), 2, "two held: {text}");
        assert_eq!(text.matches('○').count(), 6, "six missing: {text}");
        assert!(text.contains("2/8"));
        assert!(
            text.contains("●○ ○") || text.contains("] ["),
            "two groups of four: {text}"
        );
        // Glyph *and* colour, so a monochrome or colour-blind read still works.
        let held = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "●")
            .expect("a held marker");
        let missing = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "○")
            .expect("a missing marker");
        assert_ne!(held.style.fg, missing.style.fg);
    }

    /// **The brackets go before the next gym does.** They are decoration
    /// around markers that already differ in glyph and colour, and thirteen
    /// columns of them is exactly what would stop the actionable half of the
    /// deleted footer fitting on this row at the design width.
    #[test]
    fn the_badge_row_degrades_rather_than_wrapping() {
        let st = TrainerState {
            badges: 0b0000_0011,
            ..Default::default()
        };
        // At the design width the next gym is there and the brackets are not.
        let design = plain(&[badge_row(&st, 38)])[0].clone();
        assert!(design.contains("Lt.Surge"), "{design}");
        assert!(!design.contains("[●]"), "brackets over the gym: {design}");
        // Given enough room, both.
        let wide = plain(&[badge_row(&st, 60)])[0].clone();
        assert!(wide.contains("[●]") && wide.contains("Lt.Surge"), "{wide}");
        // Narrower still: the leader's name outlives the word "next".
        let mid = plain(&[badge_row(&st, 26)])[0].clone();
        assert!(!mid.contains("next"), "{mid}");
        assert!(mid.contains("Lt.Surge"), "{mid}");
        let all = TrainerState {
            badges: 0xFF,
            ..Default::default()
        };
        let narrow = plain(&[badge_row(&all, 20)])[0].clone();
        assert!(!narrow.contains("[●]"), "brackets go first: {narrow}");
        assert!(narrow.contains("●●●●"), "{narrow}");
        let tiny = plain(&[badge_row(&all, 5)])[0].clone();
        assert!(tiny.contains("8/8"), "the count is what cannot go: {tiny}");
    }

    #[test]
    fn the_slot_row_shows_empty_slots_and_marks_the_active_one() {
        let mut st = filled_state();
        st.party[2].hp = 0;
        let line = slot_row(&st, Some(1), 38);
        let text = plain(std::slice::from_ref(&line))[0].clone();
        // Three alive-or-fainted, three empty; the fainted one is hollow too.
        assert_eq!(text.matches('●').count(), 2, "{text}");
        assert_eq!(text.matches('○').count(), 4, "{text}");
        let underlined = line
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .count();
        assert_eq!(underlined, 1, "exactly the active slot is underlined");
    }

    #[test]
    fn the_trainer_line_drops_the_id_before_the_money() {
        let st = filled_state();
        let wide = plain(&header_lines(&st, "Pokémon Blue", 38))[0].clone();
        assert!(wide.contains("ID"), "{wide}");
        assert!(wide.contains("3,000"), "{wide}");
        let narrow = plain(&header_lines(&st, "Pokémon Blue", 20))[0].clone();
        assert!(!narrow.contains("ID"), "the ID goes first: {narrow}");
        assert!(narrow.contains("3,000"), "the money stays: {narrow}");
        let tiny = plain(&header_lines(&st, "Pokémon Blue", 11))[0].clone();
        assert!(tiny.contains("BLUE"), "the name never goes: {tiny}");
    }

    /// **No two stats may share a label at any width.** The narrow tier used
    /// one letter each, which prints `S55 S29` — Speed and Special, and no way
    /// to tell which is which. An ambiguous label is worse than a missing one,
    /// and this tier was only two columns cheaper than the full one.
    #[test]
    fn the_four_stats_never_share_a_label_at_any_width() {
        for w in 8usize..=60 {
            let text = plain(&[stat_line(39, 112, 55, 29, w)])[0].clone();
            assert!(text.chars().count() <= w, "overflows at {w}: {text:?}");
            // The labels are whatever is left when the digits and spaces go.
            let labels: Vec<&str> = text
                .split_whitespace()
                .filter(|t| t.chars().any(|c| c.is_ascii_alphabetic()))
                .collect();
            let mut seen = Vec::new();
            for l in &labels {
                assert!(
                    !seen.contains(l),
                    "two stats share the label {l:?} at width {w}: {text:?}"
                );
                seen.push(l);
            }
        }
    }

    #[test]
    fn a_long_type_is_abbreviated_rather_than_truncated() {
        let text = |m: &Mon, w: usize| -> String {
            type_spans(m, w)
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        let m = mon(20, 41);
        assert_eq!(text(&m, 12), "GRASS/POISON");
        assert_eq!(text(&m, 8), "GRA/POI");
        let single = Mon {
            types: ("ELECTRIC".into(), "ELECTRIC".into()),
            ..mon(20, 41)
        };
        assert_eq!(text(&single, 8), "ELECTRIC");
        assert_eq!(text(&single, 4), "ELE");
    }

    /// **Each half of a dual type gets its own colour.** One uniform colour
    /// over `GRASS/POISON` says "this is a type label"; two say which two, and
    /// that is the whole reason the panel spends colour on types at all.
    #[test]
    fn a_dual_type_is_two_colours_and_survives_abbreviation() {
        let _g = panel::test_lock();
        panel::set_depth(panel::Depth::Indexed);
        let m = mon(20, 41); // GRASS/POISON
        for w in [12usize, 8] {
            let spans = type_spans(&m, w);
            assert_eq!(spans.len(), 3, "name, separator, name at width {w}");
            assert_eq!(spans[0].style.fg, Some(panel::type_color("GRASS")));
            assert_eq!(spans[2].style.fg, Some(panel::type_color("POISON")));
            assert_ne!(spans[0].style.fg, spans[2].style.fg);
        }
        // Not even the abbreviations fit: the first type survives whole and
        // keeps its colour, rather than half of each losing both.
        let spans = type_spans(&m, 4);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style.fg, Some(panel::type_color("GRASS")));
    }

    /// Every glyph the panel draws must be one column wide, or the fixed-width
    /// columns are a fiction and the alignment tests are measuring nothing.
    #[test]
    fn every_glyph_the_panel_uses_is_single_width() {
        let st = battling();
        let mut lines = header_lines(&st, "Pokémon Blue", 38);
        for f in focus_ring(&st) {
            lines.extend(body_lines(&st, Some(f), false, 38));
            lines.extend(body_lines(&st, Some(f), true, 38));
        }
        for l in &lines {
            for ch in plain(std::slice::from_ref(l))[0].chars() {
                // Anything outside these ranges has to be justified before it
                // is used: a double-width or combining character breaks every
                // column in the panel.
                let ok = ch.is_ascii()
                    || matches!(
                        ch,
                        '─' | '│'
                            | '█'
                            | '▌'
                            | '░'
                            | '●'
                            | '○'
                            | '▸'
                            | '★'
                            | '¥'
                            | '…'
                            | '—'
                            | 'é'
                            // Added by the redesign: a field separator that
                            // does not read as a table border, and the short
                            // form of the next-gym marker.
                            | '·'
                            | '→'
                            | '▚'
                    );
                assert!(ok, "unexpected glyph {ch:?} ({:#06x})", ch as u32);
            }
        }
    }

    #[test]
    fn one_hit_point_never_renders_as_an_empty_bar() {
        // The bar and the number must never disagree: 1/41 rounds to zero cells
        // but has to show something.
        let m = mon(1, 41);
        assert!(hp_bar(m.hp_fraction(), 8).starts_with('█'));
        // Fainted really is empty.
        assert_eq!(hp_bar(0.0, 8), "░░░░░░░░");
        // And a full bar is full.
        assert_eq!(hp_bar(1.0, 8), "████████");
        // Every bar is the width asked for, so columns line up.
        for n in 0..=41u16 {
            assert_eq!(hp_bar(mon(n, 41).hp_fraction(), 10).chars().count(), 10);
        }
    }

    #[test]
    fn hp_colour_has_three_bands() {
        assert_eq!(hp_color(1.0), Color::Green);
        assert_eq!(hp_color(0.51), Color::Green);
        assert_eq!(hp_color(0.5), Color::Yellow);
        assert_eq!(hp_color(0.21), Color::Yellow);
        assert_eq!(hp_color(0.2), Color::Red);
        assert_eq!(hp_color(0.0), Color::Red);
    }

    #[test]
    fn an_empty_party_says_so_rather_than_rendering_nothing() {
        let st = TrainerState {
            in_game: true,
            ..Default::default()
        };
        let lines = party_lines(&st, Some(0), 38, false);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn the_detail_view_marks_unknown_stat_experience_rather_than_zero() {
        // A battle-struct Pokémon has no stat experience; showing 0 would be a
        // lie, so it must show the unknown glyph.
        let text = plain(&detail_lines(&mon(20, 41), false, None, 38)).join("\n");
        assert!(text.contains(UNKNOWN), "unknown stat exp is marked");
        assert!(text.contains("HIDDEN POWER"));
        assert!(text.contains("DV"));
    }

    fn encounter(hi: u8, lo: u8) -> WildEncounter {
        WildEncounter {
            species: 0x24,
            dex: 16,
            name: "PIDGEY".into(),
            level: 3,
            dvs: Dvs::from_bytes(hi, lo),
            kind: BattleType::Normal,
        }
    }

    fn banner_text(enc: &WildEncounter, width: usize) -> String {
        encounter_banner(enc, width)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect()
    }

    #[test]
    fn the_banner_never_says_a_gen_1_pokemon_is_shiny() {
        // The entire risk of this feature: Gen 1 has no shiny Pokémon, so any
        // wording that could be read as "this one is shiny" sends someone
        // hunting for a colour the cartridge cannot draw. Every width of every
        // verdict must carry the qualifier or say nothing at all.
        for (hi, lo) in [(0xEAu8, 0xAAu8), (0xFF, 0xFF), (0x00, 0x00)] {
            let enc = encounter(hi, lo);
            for w in 8..=200usize {
                let t = banner_text(&enc, w);
                assert!(t.chars().count() <= w, "banner overflows {w} cells: {t:?}");
                if !t.to_uppercase().contains("SHINY") {
                    continue;
                }
                let upper = t.to_uppercase();
                assert!(
                    upper.contains("IF TRADED")
                        || upper.contains("SHINY→GEN2")
                        || upper.contains("NOT BE SHINY")
                        || upper.contains("NOT SHINY"),
                    "at width {w} the banner claims shininess without the \
                     \"if traded\" qualifier: {t:?}"
                );
            }
        }
    }

    #[test]
    fn a_shiny_verdict_is_unmistakable_and_carries_the_numbers() {
        let enc = encounter(0xEA, 0xAA); // ATKDEFDV_SHINY / SPDSPCDV_SHINY
        let t = banner_text(&enc, 200);
        assert!(t.contains('★'), "{t}");
        assert!(t.contains("WOULD BE SHINY IF TRADED TO GEN 2"), "{t}");
        assert!(t.contains("PIDGEY L3"), "{t}");
        assert!(t.contains("DV 0/14/10/10/10"), "{t}");
        assert!(t.contains("Gold/Silver"), "{t}");
        // And it is styled to shout, not to blend in.
        let style = encounter_banner(&enc, 200).spans[0].style;
        assert_eq!(style.bg, Some(Color::LightYellow));
        // A shiny row is filled edge to edge, so it reads as a bar rather than
        // a sentence someone can skim past.
        assert_eq!(banner_text(&enc, 200).chars().count(), 200);
    }

    #[test]
    fn an_ordinary_encounter_still_shows_its_dvs_and_says_no() {
        // Perfect DVs: the case a "high DVs" heuristic gets wrong, and the one
        // that would cost a Ball.
        let t = banner_text(&encounter(0xFF, 0xFF), 160);
        assert!(t.contains("DV 15/15/15/15/15"), "{t}");
        assert!(t.contains("would NOT be shiny if traded to Gen 2"), "{t}");
        assert!(t.contains("0/3 tens"), "{t}");
        // Quiet, so it does not compete with the game.
        let style = encounter_banner(&encounter(0xFF, 0xFF), 160).spans[0].style;
        assert_eq!(style.bg, None);
    }

    #[test]
    fn a_safari_encounter_is_labelled_as_one() {
        let mut enc = encounter(0xEA, 0xAA);
        enc.kind = BattleType::Safari;
        assert!(banner_text(&enc, 160).contains("SAFARI"));
    }

    #[test]
    fn the_battle_panel_shows_the_opponents_dvs_and_names_a_demo_battle() {
        let mut o = mon(20, 41);
        o.dvs = Dvs::from_bytes(0xEA, 0xAA);
        let st = TrainerState {
            battle: BattleKind::Wild,
            battle_type: BattleType::Normal,
            opponent: Some(o.clone()),
            ..Default::default()
        };
        let text = plain(&opponent_lines(&st, false, 38)).join("\n");
        assert!(text.contains("DV "), "{text}");
        assert!(text.contains("0/14/10/10/10"), "{text}");
        assert!(text.contains("would be SHINY if traded to Gen 2"), "{text}");

        // The old man's catching demonstration is a wild battle whose Pokémon
        // can never be the player's, and the panel must say so rather than
        // imply a Ball is worth spending.
        let st = TrainerState {
            battle: BattleKind::Wild,
            battle_type: BattleType::OldMan,
            opponent: Some(o),
            ..Default::default()
        };
        let text = plain(&opponent_lines(&st, false, 38)).join("\n");
        assert!(text.contains("wild (demo)"), "{text}");
    }

    fn with_team() -> TrainerState {
        let mut st = battling();
        st.enemy_team = vec![
            TeamMon {
                species: 0xB0,
                name: "GEODUDE".into(),
                level: 12,
                hp: 0,
                max_hp: 34,
                known: true,
            },
            TeamMon {
                species: 0xB1,
                name: "ONIX".into(),
                level: 14,
                hp: 40,
                max_hp: 40,
                known: true,
            },
            TeamMon::default(),
        ];
        st
    }

    /// **The opposing trainer's roster is text now, and that is a declared
    /// trade rather than an omission.**
    ///
    /// It used to be a strip of pictures across the panel, granted only in a
    /// trainer battle on a panel wide *and* tall enough. There is one picture
    /// budget and the maintainer's ask was the player's own six, always — so the
    /// roster keeps the marker line and the names, which is exactly the
    /// fallback it already took on most window sizes, and it keeps them
    /// **unconditionally** instead. Fainted, standing and unreadable stay
    /// three different glyphs.
    #[test]
    fn the_opposing_roster_is_still_reported_at_every_width() {
        let st = with_team();
        for w in 20usize..=60 {
            let body =
                compose_body(&st, Some(Focus::Opponent), false, w, Pictures::Image, 80);
            let text = plain(&body.lines).join("\n");
            assert!(
                text.contains("TEAM"),
                "{w} columns: the roster vanished:\n{text}"
            );
        }
        // …and in the detail view, where the strip was refused outright.
        let body =
            compose_body(&st, Some(Focus::Opponent), true, 38, Pictures::Image, 80);
        let text = plain(&body.lines).join("\n");
        assert!(
            text.contains("TEAM"),
            "no roster in the detail view:\n{text}"
        );
        assert!(text.contains("GEODUDE"), "and it names what is coming");
    }

    /// Beaten and unreadable are opposite facts and must not share a glyph.
    #[test]
    fn the_team_line_distinguishes_fainted_from_unknown() {
        let st = with_team();
        let text = plain(&[team_line(&st, 38, true)])[0].clone();
        assert!(text.starts_with("TEAM "), "{text}");
        assert_eq!(text.matches('●').count(), 1, "one still standing: {text}");
        assert_eq!(text.matches('○').count(), 1, "one fainted: {text}");
        assert!(text.contains(UNKNOWN), "one unreadable: {text}");
        assert!(text.contains("1 left"), "{text}");
        // And it is reachable from the opponent section, which is where a
        // reader looks during a battle.
        let body = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        assert!(body.contains("TEAM"), "{body}");
    }

    /// A roster that did not read back says so. Showing nothing would read as
    /// "this trainer has one Pokémon", which is a lie a tracker must not tell.
    #[test]
    fn an_unreadable_roster_says_unknown_rather_than_nothing() {
        let mut st = battling();
        st.enemy_team_unknown = true;
        let body = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        assert!(body.contains(&format!("TEAM {UNKNOWN}")), "{body}");
    }

    /// A wild encounter has no trainer and therefore no roster — the two cases
    /// must not look the same.
    #[test]
    fn a_wild_encounter_has_no_team_row() {
        let mut st = with_team();
        st.battle = BattleKind::Wild;
        st.trainer_name.clear();
        let body = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        assert!(!body.contains("TEAM"), "{body}");
        assert!(body.contains("wild"), "the encounter is named: {body}");
        assert_eq!(line_width(&team_line(&st, 38, true)), 0);
    }

    /// A `6` that is really a debuffed `10` must not read like a Pokémon that
    /// was born weak. Both sides get a line, and direction is carried by colour
    /// as well as by the sign.
    #[test]
    fn stat_stages_are_shown_for_both_sides() {
        let st = battling();
        let text = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        assert!(text.contains("MOD ATK-1"), "the opponent's drop: {text}");
        assert!(text.contains("YOU DEF-2"), "and your own: {text}");
        let line = stat_mod_line(st.opponent.as_ref().unwrap().stat_mods, "MOD", 38)
            .expect("a modified opponent has a line");
        let down = line
            .spans
            .iter()
            .find(|s| s.content.contains("ATK-1"))
            .expect("the entry");
        assert_eq!(down.style.fg, Some(Color::Red), "lowered reads red");
        let up = stat_mod_line(
            Some(terminalgb_plugin_trainer::derived::StatMods::from_bytes([
                9, 7, 7, 7, 7, 7,
            ])),
            "MOD",
            38,
        )
        .unwrap();
        assert_eq!(
            up.spans
                .iter()
                .find(|s| s.content.contains("ATK+2"))
                .unwrap()
                .style
                .fg,
            Some(Color::Green),
            "raised reads green"
        );
    }

    /// Stages do not survive a switch or the end of a battle, so a stale one on
    /// screen would be worse than none. The party copy never carries them, and
    /// with no battle there is no line at all.
    #[test]
    fn stat_stages_never_appear_outside_a_battle() {
        let st = filled_state();
        assert!(st.party.iter().all(|m| m.stat_mods.is_none()));
        let text = plain(&body_lines(&st, Some(Focus::Party(0)), false, 38)).join("\n");
        assert!(!text.contains("MOD"), "{text}");
        assert!(!text.contains("YOU"), "{text}");
        let detail = plain(&body_lines(&st, Some(Focus::Party(0)), true, 38)).join("\n");
        assert!(!detail.contains("MOD "), "{detail}");
        // An all-neutral set is nothing to report either.
        assert!(stat_mod_line(
            Some(terminalgb_plugin_trainer::derived::StatMods::from_bytes(
                [7; 6]
            )),
            "MOD",
            38
        )
        .is_none());
        assert!(stat_mod_line(None, "MOD", 38).is_none());
    }

    /// The detail view of the Pokémon that is *in* the fight shows its stages,
    /// even though the party structure it is otherwise read from has none.
    #[test]
    fn the_active_party_members_detail_borrows_the_battle_copys_stages() {
        let st = battling();
        let active = active_slot(&st).expect("an active slot");
        let text =
            plain(&body_lines(&st, Some(Focus::Party(active)), true, 38)).join("\n");
        assert!(text.contains("MOD DEF-2"), "{text}");
        // A party member that is not out has nothing to report.
        let other = (0..st.party.len()).find(|i| *i != active).unwrap();
        let text =
            plain(&body_lines(&st, Some(Focus::Party(other)), true, 38)).join("\n");
        assert!(!text.contains("MOD "), "{text}");
    }

    fn wild(catch_rate: u8, hp: u16, status: u8) -> TrainerState {
        let mut o = mon(hp, 41);
        o.name = "PIDGEY".into();
        o.status = Status(status);
        TrainerState {
            in_game: true,
            battle: BattleKind::Wild,
            opponent: Some(o),
            catch_rate: Some(catch_rate),
            ..Default::default()
        }
    }

    /// The catch chance is a wild-encounter thing only. A trainer's Pokémon
    /// cannot be caught, so a percentage there would be worse than silence.
    #[test]
    fn the_catch_chance_is_only_shown_for_wild_encounters() {
        let text = plain(&body_lines(
            &wild(45, 41, 0),
            Some(Focus::Opponent),
            false,
            38,
        ))
        .join("\n");
        assert!(text.contains("POKé"), "{text}");
        assert!(text.contains("GREAT") && text.contains("ULTRA"), "{text}");

        let mut st = battling(); // a trainer battle
        st.catch_rate = Some(45);
        assert!(!st.catchable());
        let text = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        assert!(!text.contains("POKé "), "{text}");
        // And with no battle at all there is nothing to catch.
        assert!(catch_lines(&filled_state(), 38).is_empty());
    }

    /// The number has to be seen to move — that was the request — so the
    /// direction marker is part of the line and points the right way.
    #[test]
    fn the_catch_line_shows_which_way_the_number_moved() {
        let mut st = wild(45, 41, 0);
        st.catch_delta = Some(0.05);
        assert!(plain(&catch_lines(&st, 38))[0].contains('▲'));
        st.catch_delta = Some(-0.05);
        assert!(plain(&catch_lines(&st, 38))[0].contains('▼'));
        // No movement, and the first sample of an encounter, get no marker —
        // an arrow that means nothing is worse than none.
        st.catch_delta = Some(0.0);
        assert!(!plain(&catch_lines(&st, 38))[0].contains('▲'));
        st.catch_delta = None;
        let l = plain(&catch_lines(&st, 38))[0].clone();
        assert!(!l.contains('▲') && !l.contains('▼'), "{l}");
    }

    /// The comparison across balls is the decision being made, so it survives
    /// as long as it can; only when it truly cannot fit does it fall back to
    /// the best single number.
    #[test]
    fn the_catch_line_keeps_the_comparison_as_long_as_it_fits() {
        let st = wild(45, 41, 0);
        for w in 8usize..=60 {
            for l in &catch_lines(&st, w) {
                assert!(line_width(l) <= w, "width {w}: {:?}", plain(&[l.clone()]));
            }
        }
        assert!(plain(&catch_lines(&st, 36))[0].contains("ULTRA"));
        let narrow = plain(&catch_lines(&st, 16))[0].clone();
        assert!(narrow.starts_with("CATCH"), "{narrow}");
    }

    /// Below a quarter of maximum HP the routine's HP term saturates and more
    /// chip damage does nothing. Saying so is the difference between a number
    /// and advice.
    #[test]
    fn the_panel_says_when_more_damage_will_not_help() {
        let text = plain(&catch_lines(&wild(45, 1, 0), 38)).join("\n");
        assert!(text.contains("as low as it counts"), "{text}");
        let text = plain(&catch_lines(&wild(45, 41, 0), 38)).join("\n");
        assert!(!text.contains("as low as it counts"), "{text}");
    }

    #[test]
    fn no_battle_produces_no_opponent_section() {
        let st = TrainerState::default();
        assert!(opponent_lines(&st, false, 38).is_empty());
        let text = plain(&body_lines(
            &filled_state(),
            Some(Focus::Party(0)),
            false,
            38,
        ))
        .join("\n");
        assert!(!text.contains("OPPONENT"));
        // The badge footer is gone — its two rows restated the eight markers
        // in the header and named the next gym, which now rides on the marker
        // row itself. Check the fact survived the rows that carried it.
        let head = plain(&header_lines(&filled_state(), "Pokémon Blue", 38)).join("\n");
        assert!(head.contains("2/8"), "{head}");
        assert!(head.contains("Lt.Surge"), "the next gym survived: {head}");
    }

    #[test]
    fn money_is_grouped_for_reading() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(999999), "999,999");
    }
}

#[cfg(test)]
mod link_line_tests {
    use super::*;

    /// The three states that change a player's answer to "should I walk into
    /// the Cable Club?" must be distinguishable at a glance.
    #[test]
    fn the_link_line_separates_nobody_connected_and_trading() {
        let none = plain(&[link_line("no one connected", false, 40)])[0].clone();
        let idle =
            plain(&[link_line("connected to RED · POKEMON BLUE", false, 40)])[0].clone();
        let busy = plain(&[link_line("connected to RED — TRADING", true, 40)])[0].clone();
        assert!(none.contains("LINK") && none.contains("no one"));
        assert!(idle.contains("RED") && !idle.contains("TRADING"));
        assert!(busy.contains("TRADING"));
        assert_ne!(none, idle);
        assert_ne!(idle, busy);
    }

    /// The panel is dense and the maintainer splits it narrow. The row must lose
    /// its *tail*, never its label — a clipped line that no longer says LINK is
    /// indistinguishable from some other row.
    #[test]
    fn the_link_line_keeps_its_label_at_every_width() {
        for w in 6..=48usize {
            let line = plain(&[link_line("connected to RED · POKEMON BLUE", true, w)])[0]
                .clone();
            assert!(
                line.chars().count() <= w,
                "width {w}: {line:?} overflows the panel"
            );
            assert!(
                line.starts_with("LINK") || line.starts_with("LIN") || w < 5,
                "width {w}: the label was clipped away: {line:?}"
            );
        }
    }

    /// A cable that is not there costs the panel nothing: the frontend passes
    /// `None` and no row is produced at all (see `App::render_trainer`).
    #[test]
    fn an_unplugged_cable_is_dim_not_loud() {
        let l = link_line("cable unplugged (W)", false, 40);
        assert_ne!(
            l.spans[1].style.fg,
            Some(Color::Green),
            "an unplugged cable must not read as connected"
        );
    }
}

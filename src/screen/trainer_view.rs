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
//! * **The sprites are themselves images**, so they get their own rectangles and
//!   their own image ids, and the same rule applies to them.
//!
//! # The shape of the panel
//!
//! ```text
//! ┌ Trainer · d detail · Tab next ───────┐
//! │BLUE · ID 62800 · ¥3,000 · 0:01:41    │  identity, one row
//! │●●○○ ○○○○ 2/8 · next Lt.Surge         │  badges, one row
//! │  [ six small sprites, one strip ]    │  THE TEAM, always
//! │  ▸1    2    3    4    ·    ·         │  the rail: slot ↔ sprite
//! │TEAM ─────────────────────────── 4/6  │
//! │▸1 SPARK      L 12 █████ ELECTRIC     │  one row each
//! │ 2 RATTY      L  9 █░░░░ NORMAL   PSN │
//! └──────────────────────────────────────┘
//! ```
//!
//! Three things about that are deliberate, and they are the whole redesign:
//!
//! 1. **Every party member is on screen at all times, with a picture.** The six
//!    sprites are one composed strip (see `compose_party_strip` in
//!    [`crate::screen::tui`]), so six pictures cost what one did — one image id,
//!    one placement, one encode. The [`rail_line`] under it carries the slot
//!    numbers in the same six columns, which is what ties a face to a row of the
//!    list below.
//! 2. **The sprites are small.** [`strip_rows`] gives the band three rows at the
//!    design width against the eleven a single portrait used to take, so each
//!    Pokémon lands in about six cells square — enough to identify, not enough
//!    to perform. The picture identifies; the table informs.
//! 3. **No centred banners.** A centred `[ PARTY ]` rule is a row of decoration
//!    with a word in the middle of it. [`panel_theme::section`] anchors the label
//!    left, where the eye already is, and puts a *value* on the right-hand end,
//!    so the same row names the block and reports on it.
//!
//! # Degrading as the panel narrows and shortens
//!
//! Every section takes the width it is given and drops its **least valuable
//! column first** rather than wrapping into mush. [`party_cols`] spends the
//! width it has in a fixed priority order — name, HP bar, status, type, the
//! shiny mark, then the exact HP numbers, and only then does it let the columns
//! grow — so every row in a panel makes the same decisions and the columns line
//! up down the page. `no_line_overflows_at_any_width` renders every section at
//! every width from 8 to 60 and asserts nothing exceeds it.
//!
//! Vertically the order of sacrifice is: the opposing side's strip, then the
//! party strip, then rows off the bottom. **The pictures go before the numbers**
//! — a picture is decoration, a number is the reason the panel exists. When the
//! party strip cannot be granted, the party *list* is still there in full; what
//! is lost is the faces, and [`crate::screen::tui`] says so in the panel title
//! rather than leaving the reader to notice.
//!
//! # Colour
//!
//! All of it comes from [`panel_theme`] — there is no `Color::` literal in this
//! file. Types are coloured by type, HP by band, status by how much it changes
//! the next decision, stat stages by direction. Every one of them is *also*
//! spelled out, so colour is reinforcement and never the fact; see
//! [`panel_theme`] for the rules and the tests that hold it to them.
//!
//! # Glyphs
//!
//! Only characters already used elsewhere in this frontend: box drawing
//! (`─ │ ┌`), the block elements `█ ▌ ░`, the geometric shapes `● ○ ▸ ▲ ▼ ·`,
//! and `★`. All of them come from fonts a terminal already needs for the rest
//! of the UI — nothing here wants a Nerd Font, and nothing is double-width.
//!
//! # What earns space, and what is behind a key
//!
//! Taken from the [Ironmon Tracker](https://github.com/besteon/Ironmon-Tracker)
//! (MIT; read for its choices, not its code — it is Lua against a different
//! emulator and targets Gen 3):
//!
//! * **Every pixel earns its place**: sprite, stats, types, moves and their
//!   PP/power/accuracy all fit in one small box, with no scrolling and no
//!   paging. Ours tabulates the same way — [`stat_table`] and [`move_table`]
//!   are shared by the detail view and the battle block so a number is always
//!   in the same column.
//! * **HP is colour-coded by fraction** — green, then amber under a half, then
//!   red under a fifth. It is the fastest thing to read on the panel and costs
//!   nothing.
//! * **A single "unknown" glyph.** The tracker uses `?` everywhere for
//!   not-yet-known data; a blank reads as "zero" and a zero is a lie. Same here:
//!   [`UNKNOWN`]. An *absent* value (an empty move slot, a status move's power)
//!   gets `—` instead, because empty and unknown are opposite facts.
//!
//! Deliberately different for Gen 1: there are no abilities, natures or held
//! items to show, "special" is one stat rather than two, and DVs go to 15 rather
//! than IVs to 31 — so the detail view spends its space on DVs, the invisible
//! stat experience, and the derived values in
//! [`terminalgb_plugin_trainer::derived`].

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::screen::panel_theme as theme;
use terminalgb_plugin_trainer::encounter::WildEncounter;
use terminalgb_plugin_trainer::gen1::{BattleKind, BattleType};
use terminalgb_plugin_trainer::gen1::{Mon, MoveSlot, BADGE_LEADERS, BADGE_NAMES};
use terminalgb_plugin_trainer::{TeamMon, TrainerState};

pub use crate::screen::panel_theme::hp_color;

/// Shown wherever a value is not known yet, so a blank never reads as a zero.
pub const UNKNOWN: &str = "?";

/// Shown wherever a value is genuinely *absent* — an empty move slot, the power
/// of a status move. The opposite fact from [`UNKNOWN`], and it must not share
/// its glyph.
pub const ABSENT: &str = "—";

/// Width of the trainer panel in cells. Two border columns plus the 38 the
/// party rows are laid out for.
pub const PANEL_W: u16 = 40;

/// Narrowest panel worth drawing at all. Below this the party rows lose the
/// level as well as the bar and the header keeps only a name, which is less
/// information than the space costs — so the frontend gives the width back to
/// the game picture instead.
pub const PANEL_MIN_W: u16 = 22;

/// Slots in a party, and therefore columns in the strip and the rail.
pub const SLOTS: usize = terminalgb_plugin_trainer::gen1::PARTY_LENGTH;

/// Rows the party sprite strip gets in a panel `inner` columns wide, **plus one
/// for the rail beneath it**.
///
/// The old panel gave a single portrait eleven rows and three tenths of the
/// width, on the theory that the sprite was the one part of the panel that
/// could genuinely scale. It could, and it did, and the result was a picture
/// that occupied more than half the panel while the team it belonged to was two
/// lines at the bottom. Six pictures at a twelfth of the width each is the same
/// idea spent correctly: it still grows with the panel, but what grows is *how
/// many faces you can make out*, not how large one of them is.
///
/// Three rows at the design width puts each Pokémon in roughly six cells square
/// — about its native 56×56 pixels at a normal cell size, so it is drawn at 1:1
/// rather than magnified. The ceiling stops a very tall panel turning the strip
/// back into a poster.
pub fn strip_rows(inner: u16) -> u16 {
    (inner / 12).clamp(3, 6)
}

/// Rows the trainer header always takes: identity, then badges.
///
/// Two, against the five it used to take — two centred `[ BANNER ]` rules, two
/// field lines and a badge row. The rules are gone (see the module docs) and the
/// badge *names* went with them: the eight markers are positional, so the row
/// already says which badges are held, and `next <leader>` anchors the order.
pub const HEADER_ROWS: u16 = 2;

/// Rows of text that must survive before either strip is allowed any. The party
/// list is the point; the pictures are the decoration.
const MIN_BODY_ROWS: u16 = 6;

/// Rows the opposing side's strip takes.
pub const FOE_ROWS: u16 = 3;

/// Narrowest panel that gets the opposing side's strip. Below this a six-strong
/// roster would be under four cells per Pokémon, which is a smudge.
pub const FOE_MIN_W: u16 = 24;

/// Where the panel's pieces go inside its rectangle.
///
/// Everything except [`frame`](PanelLayout::frame) is *inside* the border, so
/// the panel reads as one object rather than as a stack of boxes.
///
/// The order down the panel is **header, our team, the rail, their side, the
/// numbers**. That is what puts each picture band next to its own caption: the
/// rail names our six, and the `FOE` section header — the first line of the
/// body during a battle — names theirs.
pub struct PanelLayout {
    /// The single bordered block, drawn over the whole panel rectangle.
    pub frame: Rect,
    /// The trainer header: name, ID, money, play time, badges.
    pub header: Rect,
    /// The six party sprites, as one composed strip. Empty (`height == 0`) when
    /// there is no room — the text must still be readable on a short terminal.
    pub party: Rect,
    /// One row of slot markers, in the same six columns as [`party`]. Granted
    /// only when the strip is, because on its own it is a row of numbers
    /// pointing at nothing.
    ///
    /// [`party`]: PanelLayout::party
    pub rail: Rect,
    /// The other side of a fight, as one strip: a trainer's whole roster, or the
    /// single Pokémon of a wild encounter. Empty (`height == 0`) out of battle,
    /// on a narrow or short panel, or when the roster did not read back — in
    /// which case the `FOE` section carries the markers instead.
    pub foe: Rect,
    /// The party list, or the detail view.
    pub body: Rect,
}

/// Split the panel rectangle. Never returns overlapping rectangles, and never
/// returns a picture area that the text would be drawn over.
pub fn layout(area: Rect, want_party: bool) -> PanelLayout {
    layout_with_foe(area, want_party, false, MIN_BODY_ROWS)
}

/// [`layout`] plus the opposing side's strip.
///
/// `body_rows` is how many rows of text the body actually wants — the caller
/// has already composed them, so it costs nothing to ask. **Both strips are
/// granted space only if the text still fits after them.** That is what stops a
/// battle, whose `FOE` block is a dozen rows on its own, from pushing the party
/// off the bottom: the pictures are the decoration and they go first, and the
/// numbers stay.
///
/// The opposing strip is **the first to be refused**: our own team is the thing
/// the captain asked to always be able to see, and a roster that does not fit is
/// still reported as markers on one line inside `FOE`.
pub fn layout_with_foe(
    area: Rect,
    want_party: bool,
    want_foe: bool,
    body_rows: u16,
) -> PanelLayout {
    // Inside the border. A panel too small to have an interior gets zero-sized
    // pieces rather than rectangles that escape it.
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let header_h = HEADER_ROWS.min(inner.height);
    let rest_h = inner.height.saturating_sub(header_h);
    let need = body_rows.max(MIN_BODY_ROWS);
    // The strip and its rail are granted together: a rail with no faces above
    // it is a row of numbers pointing at nothing.
    let band = strip_rows(inner.width) + 1;
    let party_h = if want_party && rest_h >= band + need {
        band
    } else {
        0
    };
    let after_party = rest_h.saturating_sub(party_h);
    let foe_h = if want_foe && inner.width >= FOE_MIN_W && after_party >= FOE_ROWS + need
    {
        FOE_ROWS
    } else {
        0
    };
    let strip_h = party_h.saturating_sub(party_h.min(1));
    let y = inner.y + header_h;
    PanelLayout {
        frame: area,
        header: Rect::new(inner.x, inner.y, inner.width, header_h),
        party: Rect::new(inner.x, y, inner.width, strip_h),
        rail: Rect::new(inner.x, y + strip_h, inner.width, party_h.min(1)),
        foe: Rect::new(inner.x, y + party_h, inner.width, foe_h),
        body: Rect::new(
            inner.x,
            y + party_h + foe_h,
            inner.width,
            after_party.saturating_sub(foe_h),
        ),
    }
}

impl PanelLayout {
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
    /// **The whole block moves; nothing is re-decided.** The strips were granted
    /// against the full column, so translating the finished layout cannot make
    /// either of them vanish — which re-running the split on a shorter rectangle
    /// could.
    ///
    /// When the content is taller than the column there is no slack to
    /// distribute, and the panel stays exactly where it was: full height and
    /// top-aligned, so what falls off is the **bottom**. Centring an
    /// over-height panel would push its header off the top, and the header is
    /// the part that says whose panel this is.
    pub fn centre_in(self, area: Rect, body_rows: u16) -> PanelLayout {
        let content = body_rows.min(self.body.height);
        let used = 2
            + self.header.height
            + self.party.height
            + self.rail.height
            + self.foe.height
            + content;
        if used >= area.height {
            return self;
        }
        let dy = (area.height - used) / 2;
        let shift = |r: Rect| Rect::new(r.x, r.y + dy, r.width, r.height);
        PanelLayout {
            frame: Rect::new(area.x, area.y + dy, area.width, used),
            header: shift(self.header),
            party: shift(self.party),
            rail: shift(self.rail),
            foe: shift(self.foe),
            body: Rect::new(self.body.x, self.body.y + dy, self.body.width, content),
        }
    }
}

/// The panel's single frame.
pub fn panel_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(theme::label())
}

/// A bordered block for the panel's *messages* — the unsupported-cartridge and
/// waiting-for-a-save screens, which are prose rather than sections.
pub fn body_block(title: &str) -> Block<'_> {
    panel_block(title)
}

/// Clip a composed line to `width`, span by span, keeping the styling of what
/// survives.
///
/// Every section already sizes its own columns, so at any width a person would
/// actually use this changes nothing. It is a **net**, for the widths nobody
/// designed for — a 12-column panel, a detail view whose species name is longer
/// than the box — where the alternative is a line that bleeds into the border
/// and corrupts the frame. `no_line_overflows_at_any_width` is what proves the
/// net is never load-bearing at a realistic width, because it renders the
/// sections directly.
pub fn clamp_line<'a>(l: Line<'a>, width: usize) -> Line<'a> {
    if line_width(&l) <= width {
        return l;
    }
    let mut used = 0usize;
    let mut out: Vec<Span<'a>> = Vec::with_capacity(l.spans.len());
    for s in l.spans {
        if used >= width {
            break;
        }
        let room = width - used;
        let n = s.content.chars().count();
        if n <= room {
            used += n;
            out.push(s);
        } else {
            let text: String = s.content.chars().take(room).collect();
            used = width;
            out.push(Span::styled(text, s.style));
        }
    }
    Line::from(out)
}

/// A left-anchored section header — see [`panel_theme::section`].
pub fn section<'a>(title: &str, trailer: Option<Span<'a>>, width: usize) -> Line<'a> {
    theme::section(title, trailer, width)
}

/// A plain full-width separator, used before a footnote so it reads as a
/// footnote rather than as more of the block above it.
pub fn separator<'a>(width: usize) -> Line<'a> {
    theme::divider(width)
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

/// The separator between packed fields. A middle dot rather than a box-drawing
/// pipe: the panel's vertical rules belong to the frame, and reusing the glyph
/// inside a row makes a line of text read as a table it is not.
const SEP: &str = " · ";

/// Join `fields` with [`SEP`], dropping the most-droppable until the line fits.
///
/// Labels are dim and values are not, so the eye lands on the value; that is
/// most of what makes a packed line readable at a glance rather than a wall.
fn fields_line<'a>(mut fields: Vec<Field>, width: usize) -> Line<'a> {
    let sep = SEP.chars().count();
    let total = |f: &Vec<Field>| -> usize {
        f.iter().map(Field::width).sum::<usize>() + sep * f.len().saturating_sub(1)
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
            if used + sep > width {
                break;
            }
            spans.push(Span::styled(SEP, theme::label()));
            used += sep;
        }
        let label = f.prefix();
        let lw = label.chars().count();
        if used + lw >= width {
            break;
        }
        if lw > 0 {
            spans.push(Span::styled(label, theme::label()));
            used += lw;
        }
        let value = clip(&f.value, width - used);
        used += value.chars().count();
        spans.push(Span::styled(value, theme::value()));
    }
    Line::from(spans)
}

/// A fixed-width HP bar. Always renders `width` cells so rows stay aligned —
/// alignment is most of what makes a column of numbers readable at speed.
pub fn hp_bar(fraction: f32, width: usize) -> String {
    let filled = (fraction * width as f32).round() as usize;
    let filled = filled.min(width);
    // A Pokémon on 1 HP must not render as an empty bar; that is the one case
    // where the bar and the number would disagree.
    // …but a zero-width bar is still zero cells: a panel too narrow for a bar
    // asks for none, and `max(1)` there would underflow the padding below.
    let filled = if fraction > 0.0 {
        filled.max(1).min(width)
    } else {
        0
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
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
        theme::good().add_modifier(Modifier::BOLD)
    } else if summary.starts_with("connected") {
        theme::good()
    } else {
        theme::label()
    };
    clamp_line(
        Line::from(vec![
            Span::styled("LINK ", theme::label()),
            Span::styled(summary.to_string(), style),
        ]),
        width,
    )
}

/// The trainer header: identity, then badges. Exactly [`HEADER_ROWS`] lines
/// whatever the width, so the strip below it never moves.
pub fn header_lines<'a>(state: &TrainerState, game: &str, width: usize) -> Vec<Line<'a>> {
    header_rows(state, game, width)
        .into_iter()
        .map(|l| clamp_line(l, width))
        .collect()
}

fn header_rows<'a>(state: &TrainerState, game: &str, width: usize) -> Vec<Line<'a>> {
    let (h, m, s) = state.play_time;
    vec![
        fields_line(
            vec![
                Field {
                    label: "",
                    value: state.player_name.clone(),
                    drop: 0,
                },
                // Money, then the clock, then the ID: that is the order a run
                // refers to them in, so it is the reverse order they go in.
                Field {
                    label: "",
                    value: format!("¥{}", thousands(state.money)),
                    drop: 1,
                },
                Field {
                    label: "",
                    value: format!("{h}:{m:02}:{s:02}"),
                    drop: 2,
                },
                Field {
                    label: "ID",
                    value: format!("{:05}", state.player_id),
                    drop: 3,
                },
                // The cartridge is the least interesting thing on the panel to
                // somebody already playing it, so it is the first to go — but it
                // is the only place a hack or a translation announces itself.
                Field {
                    label: "",
                    value: game.to_string(),
                    drop: 4,
                },
            ],
            width,
        ),
        badge_row(state, width),
    ]
}

/// Eight discrete badge markers in two groups of four, the count, and the next
/// gym leader.
///
/// Filled and empty differ in **both** glyph and colour (`●` yellow against `○`
/// dark grey), because a colour-blind reader or a low-contrast theme must still
/// be able to count them. The markers are positional, so this row also says
/// *which* badges are held — which is why the old `BADGES Boulder, Cascade`
/// footnote could go, and why `next <leader>` is here to anchor the order.
pub fn badge_row<'a>(state: &TrainerState, width: usize) -> Line<'a> {
    let flags = state.badge_flags();
    let count = format!("{}/8", state.badge_count());
    let got = theme::rare();

    // `●●●● ●●●●  n/8`
    let bare = 8 + 1 + 2 + count.chars().count();

    let mut spans = Vec::new();
    if width >= bare {
        for (i, &on) in flags.iter().enumerate() {
            if i == 4 {
                spans.push(Span::raw(" "));
            }
            spans.push(if on {
                Span::styled("●", got)
            } else {
                Span::styled("○", theme::label())
            });
        }
        spans.push(Span::styled(format!("  {count}"), theme::label()));
    } else {
        // No room for markers at all: the count is the part that cannot go.
        return Line::from(Span::styled(clip(&count, width), got));
    }

    let next = BADGE_LEADERS
        .iter()
        .zip(flags)
        .find(|(_, g)| !*g)
        .map(|(n, _)| *n);
    let tail = match next {
        Some(leader) => format!("next {leader}"),
        None => "all eight".to_string(),
    };
    if bare + SEP.chars().count() + tail.chars().count() <= width {
        spans.push(Span::styled(SEP, theme::label()));
        spans.push(Span::styled(tail, theme::value()));
    }
    Line::from(spans)
}

/// The rail: one row of slot markers in the same six columns as the sprite
/// strip above it.
///
/// This is the join between the picture and the table. Without it a reader has
/// six faces and a list and no way to say which is which; with it the slot
/// number sits directly under its own face and matches the number the list uses.
/// It also carries what the old `slot_row` did — an *empty* slot is visible
/// here and nowhere else, because the list below can only list what exists.
///
/// `selected` is the slot the detail view would open, `active` is the one the
/// battle engine has out. Both are marked with a glyph as well as a style.
pub fn rail_line<'a>(
    state: &TrainerState,
    selected: Option<usize>,
    active: Option<usize>,
    width: usize,
) -> Line<'a> {
    let cell = width / SLOTS;
    if cell < 2 {
        return Line::from(Span::raw(""));
    }
    let mut spans = Vec::new();
    for i in 0..SLOTS {
        let mark = if selected == Some(i) {
            "▸"
        } else if active == Some(i) {
            "▌"
        } else {
            " "
        };
        let (token, style) = match state.party.get(i) {
            None => (format!("{mark}·"), theme::label()),
            Some(m) if m.is_fainted() => (
                format!("{mark}{}", i + 1),
                theme::status_style("FNT").add_modifier(Modifier::DIM),
            ),
            Some(m) => (
                format!("{mark}{}", i + 1),
                Style::default().fg(hp_color(m.hp_fraction())),
            ),
        };
        let style = if selected == Some(i) || active == Some(i) {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };
        // Centred in its own column, so the number lands under its own face.
        let pad_l = (cell - 2) / 2;
        spans.push(Span::raw(" ".repeat(pad_l)));
        spans.push(Span::styled(token, style));
        spans.push(Span::raw(" ".repeat(cell - 2 - pad_l)));
    }
    Line::from(spans)
}

/// What the detail view is pointed at.
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
/// **The width is spent in one fixed priority order, not chosen from a table of
/// tiers.** The order is what a reader needs first: a name, then the HP bar
/// (because "how is the team doing" is this panel's first job), then the status,
/// then the type, then the shiny mark, then the exact HP numbers — which are the
/// least useful thing about a Pokémon on the bench and the most useful about the
/// one in the fight, and the fight has its own block. Only once all of those are
/// in does the leftover go back into making the name, the type and the bar
/// longer.
///
/// Because it is a pure function of the width, every row in a panel makes
/// exactly the same decisions and the columns line up down the page —
/// `party_columns_line_up_however_the_values_change` pins that, and
/// `no_line_overflows_at_any_width` pins that the sum never exceeds the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PartyCols {
    /// Nickname column.
    pub nick: usize,
    /// HP bar cells; 0 for no bar at all.
    pub bar: usize,
    /// Whether the three-character status column is drawn.
    pub status: bool,
    /// Type column; 0 for no type.
    pub ty: usize,
    /// Whether `hp/max` is spelled out beside the bar.
    pub hp: bool,
    /// Whether there is a cell left for the shiny mark.
    pub star: bool,
}

/// Cells the gutter, slot number and level column always take: `▸1 ` and
/// ` L100`.
const PARTY_FIXED: usize = 3 + 5;
/// Cells `{:>3}/{:>3}` takes, plus its leading space.
const HP_NUMBERS: usize = 8;

pub fn party_cols(width: usize) -> PartyCols {
    let mut c = PartyCols::default();
    let mut left = width.saturating_sub(PARTY_FIXED);
    // A name, however short. Gen 1 nicknames are at most ten characters
    // (`NAME_LENGTH` is eleven including the terminator), so there is no point
    // reserving more here — the leftover is worth more elsewhere.
    c.nick = left.min(10);
    left -= c.nick;
    // The HP bar, with its leading space.
    if left >= 4 {
        let b = (left - 1).min(5);
        c.bar = b;
        left -= b + 1;
    }
    // The status column, with its leading space.
    if left >= 4 {
        c.status = true;
        left -= 4;
    }
    // The type, with its leading space; three characters is the abbreviation.
    if left >= 4 {
        let t = (left - 1).min(3);
        c.ty = t;
        left -= t + 1;
    }
    // One cell for the shiny mark.
    if left >= 1 {
        c.star = true;
        left -= 1;
    }
    // The exact HP numbers.
    if left >= HP_NUMBERS {
        c.hp = true;
        left -= HP_NUMBERS;
    }
    // Everything left goes back into the columns that can use it: the type
    // first (an abbreviation is the one column that is actively worse short),
    // then the bar's resolution.
    let t = left.min(12usize.saturating_sub(c.ty));
    if c.ty > 0 {
        c.ty += t;
        left -= t;
    }
    if c.bar > 0 {
        c.bar += left.min(12usize.saturating_sub(c.bar));
    }
    c
}

/// One party member, on one line.
///
/// One line each was the other half of "I'd always want to see little versions
/// of ALL our team": six Pokémon at two lines apiece is twelve rows before the
/// panel has said anything about a battle, and it is why the party used to be
/// the thing that fell off the bottom. The second line carried the species name
/// and the exact HP — the species is in the detail view where a nickname is
/// ambiguous, and the HP numbers come back on this line as soon as the panel is
/// wide enough to hold them.
fn party_row<'a>(
    i: usize,
    m: &Mon,
    selected: bool,
    active: bool,
    c: PartyCols,
    width: usize,
) -> Line<'a> {
    let frac = m.hp_fraction();
    // One gutter cell, two meanings that never both apply to the reader at
    // once: `▸` is what `d` will open, `▌` is the Pokémon currently in the
    // fight. Focus wins the cell, because the rail above marks the active one.
    let gutter = if selected {
        "▸"
    } else if active {
        "▌"
    } else {
        " "
    };
    let nick = c.nick.min(width.saturating_sub(3));
    let mut spans = vec![
        Span::styled(
            format!("{gutter}{} ", i + 1),
            if active {
                theme::heading()
            } else {
                theme::label()
            },
        ),
        Span::styled(
            pad(&m.nickname, nick),
            if selected {
                theme::selected()
            } else {
                theme::value()
            },
        ),
    ];
    let mut used = 3 + nick;
    if used + 5 > width {
        return Line::from(spans);
    }
    spans.push(Span::styled(" L", theme::label()));
    spans.push(Span::styled(format!("{:>3}", m.level), theme::value()));
    used += 5;
    if c.bar > 0 && used + 1 + c.bar <= width {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            hp_bar(frac, c.bar),
            Style::default().fg(hp_color(frac)),
        ));
        used += 1 + c.bar;
    }
    if c.hp && used + HP_NUMBERS <= width {
        spans.push(Span::styled(
            format!(" {:>3}/{:>3}", m.hp, m.max_hp),
            Style::default().fg(hp_color(frac)),
        ));
        used += HP_NUMBERS;
    }
    if c.ty > 0 && used + 1 + c.ty <= width {
        spans.push(Span::raw(" "));
        spans.extend(type_spans(m, c.ty));
        used += 1 + c.ty;
    }
    if c.status && used + 4 <= width {
        let label = m
            .status
            .label()
            .unwrap_or(if m.is_fainted() { "FNT" } else { "" });
        spans.push(Span::styled(
            format!(" {label:<3}"),
            theme::status_style(label),
        ));
        used += 4;
    }
    if c.star && used < width {
        spans.push(if m.would_be_shiny() {
            Span::styled("★", theme::rare())
        } else {
            Span::raw(" ")
        });
    }
    Line::from(spans)
}

/// The party list, one line per Pokémon, under a section header that reports
/// how much of the team is still standing.
pub fn party_lines<'a>(
    state: &TrainerState,
    selected: Option<usize>,
    width: usize,
) -> Vec<Line<'a>> {
    let standing = state.party.iter().filter(|m| !m.is_fainted()).count();
    let trailer = if state.party.is_empty() {
        None
    } else {
        Some(Span::styled(
            format!("{standing}/{}", state.party.len()),
            if standing == state.party.len() {
                theme::label()
            } else {
                theme::bad()
            },
        ))
    };
    let mut lines = vec![section("TEAM", trailer, width)];
    if state.party.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no Pokémon yet",
            theme::label(),
        )));
        return lines;
    }
    let c = party_cols(width);
    let active = active_slot(state);
    for (i, m) in state.party.iter().enumerate() {
        lines.push(party_row(
            i,
            m,
            selected == Some(i),
            active == Some(i),
            c,
            width,
        ));
    }
    lines
}

/// A Pokémon's types, each coloured by its own type, joined by a dim `/`, in at
/// most `max` columns.
///
/// The full `GRASS/POISON` when it fits, otherwise three-letter abbreviations,
/// which stay readable where a truncation (`GRASS/POI…`) does not.
fn type_spans<'a>(m: &Mon, max: usize) -> Vec<Span<'a>> {
    let (a, b) = (m.types.0.clone(), m.types.1.clone());
    let single = b.is_empty() || a == b;
    let short = |t: &str| -> String { t.chars().take(3).collect() };
    let (a, b) = if single {
        if a.chars().count() <= max {
            (a, String::new())
        } else {
            (short(&a), String::new())
        }
    } else if a.chars().count() + 1 + b.chars().count() <= max {
        (a, b)
    } else {
        (short(&a), short(&b))
    };
    let mut spans = vec![Span::styled(clip(&a, max), theme::type_style(&a))];
    let mut used = spans[0].content.chars().count();
    if !b.is_empty() && used + 1 < max {
        spans.push(Span::styled("/", theme::label()));
        used += 1;
        let t = clip(&b, max - used);
        used += t.chars().count();
        spans.push(Span::styled(t, theme::type_style(&b)));
    }
    if used < max {
        spans.push(Span::raw(" ".repeat(max - used)));
    }
    spans
}

/// Cells a stat table's row label takes.
const STAT_LABEL_W: usize = 5;
/// Cells each stat column takes. Five digits is the range of stat experience,
/// which is the widest number in the table.
const STAT_COL_W: usize = 5;
/// The five stats, in the order every Gen 1 structure stores them.
const STAT_NAMES: [&str; 5] = ["HP", "ATK", "DEF", "SPD", "SPC"];

/// The stat block as a **table**: one column per stat, one row per kind of
/// number, every value on the same grid.
///
/// This is the single most direct thing taken from the Ironmon tracker — it
/// puts stats in a labelled column aligned on their numbers, and the result is
/// that a reader compares down a column instead of parsing a sentence. Ours had
/// `ATK 42   DEF 38   SPD 61   SPC 50` on one line and the DVs on another in a
/// different order with different spacing, so the two could not be read
/// together at all.
///
/// `exp` is `None` for a Pokémon read out of the in-battle structure, which does
/// not carry stat experience — that row then says [`UNKNOWN`] rather than zero.
pub fn stat_table<'a>(m: &Mon, width: usize) -> Vec<Line<'a>> {
    let cols = ((width.saturating_sub(STAT_LABEL_W)) / STAT_COL_W).min(STAT_NAMES.len());
    if cols < 2 {
        // Too narrow for a table; one line of the four battle stats is what
        // still fits and still says something.
        return vec![clamp_line(
            Line::from(Span::styled(
                format!(
                    "A{} D{} S{} C{}",
                    m.attack, m.defense, m.speed, m.special
                ),
                theme::value(),
            )),
            width,
        )];
    }
    let row = |label: &str, style: Style, vals: [String; 5]| -> Line<'a> {
        let mut spans = vec![Span::styled(
            format!("{label:<STAT_LABEL_W$}"),
            theme::label(),
        )];
        for v in vals.iter().take(cols) {
            spans.push(Span::styled(format!("{v:>STAT_COL_W$}"), style));
        }
        Line::from(spans)
    };
    let n = |v: u16| v.to_string();
    let mut lines = vec![
        theme::column_header(STAT_LABEL_W, &STAT_NAMES[..cols], STAT_COL_W, width),
        row(
            "stat",
            theme::value(),
            [
                n(m.max_hp),
                n(m.attack),
                n(m.defense),
                n(m.speed),
                n(m.special),
            ],
        ),
    ];
    let dv_style = if m.would_be_shiny() {
        theme::rare()
    } else {
        theme::value()
    };
    let mut dv = row(
        "DV",
        dv_style,
        [
            m.dvs.hp().to_string(),
            m.dvs.attack.to_string(),
            m.dvs.defense.to_string(),
            m.dvs.speed.to_string(),
            m.dvs.special.to_string(),
        ],
    );
    let total = format!("  {}/75", m.dv_total());
    if line_width(&dv) + total.chars().count() <= width {
        dv.spans.push(Span::styled(total, theme::label()));
    }
    lines.push(dv);
    match &m.stat_exp {
        Some(e) => lines.push(row(
            "EXP",
            theme::value(),
            [
                e.hp.to_string(),
                e.attack.to_string(),
                e.defense.to_string(),
                e.speed.to_string(),
                e.special.to_string(),
            ],
        )),
        // The in-battle structure does not carry stat experience, so during a
        // battle this is unknown rather than zero.
        None => lines.push(clamp_line(
            Line::from(vec![
                Span::styled(format!("{:<STAT_LABEL_W$}", "EXP"), theme::label()),
                Span::styled(
                    format!("{UNKNOWN} not in the battle copy"),
                    theme::label(),
                ),
            ]),
            width,
        )),
    }
    lines
}

/// The move table's columns: PP, power, accuracy.
const MOVE_PP_W: usize = 5;
const MOVE_NUM_W: usize = 4;

/// The four move slots as a **table**: name, PP, power, accuracy, type.
///
/// Power and accuracy are read from the cartridge's own `Moves` table (see
/// [`terminalgb_plugin_trainer::rom::RomTables::move_power`]); before this the
/// panel showed a name and a PP count, which is not enough to choose a move
/// with. The type is coloured by type, which is what makes "is this move going
/// to do anything" answerable without reading a word.
///
/// A status move's power and accuracy are `0` in the table, which means *no
/// such thing* rather than *unknown* — so they render as [`ABSENT`], not
/// [`UNKNOWN`].
pub fn move_table<'a>(moves: &[MoveSlot], width: usize) -> Vec<Line<'a>> {
    // Everything but the name column, plus a separating space before each.
    let tail = 1 + MOVE_PP_W + 1 + MOVE_NUM_W + 1 + MOVE_NUM_W;
    if width < tail + 6 {
        // Too narrow for a table: names and PP, which is what the panel used to
        // show at every width.
        let mut lines = vec![section("MOVES", None, width)];
        for mv in moves {
            lines.push(clamp_line(
                Line::from(vec![
                    Span::styled(pad(&mv.name, width.saturating_sub(6)), theme::value()),
                    Span::styled(
                        format!(" {:>2}/{:<2}", mv.pp, mv.pp_max),
                        pp_style(mv),
                    ),
                ]),
                width,
            ));
        }
        return lines;
    }
    let ty_w = width.saturating_sub(tail).min(8).max(0);
    let name_w = width - tail - if ty_w > 0 { ty_w + 1 } else { 0 };
    let mut head = format!(
        "{:<name_w$} {:>MOVE_PP_W$} {:>MOVE_NUM_W$} {:>MOVE_NUM_W$}",
        "MOVES", "PP", "POW", "ACC"
    );
    if ty_w > 0 {
        head.push_str(&format!(" {:<ty_w$}", "TYPE"));
    }
    let mut lines = vec![Line::from(Span::styled(head, theme::heading()))];
    for i in 0..terminalgb_plugin_trainer::gen1::NUM_MOVES {
        let Some(mv) = moves.get(i) else {
            // An empty move slot is genuinely empty, not unknown.
            lines.push(Line::from(Span::styled(ABSENT, theme::label())));
            continue;
        };
        let num = |v: u8| -> String {
            if v == 0 {
                ABSENT.to_string()
            } else {
                v.to_string()
            }
        };
        let mut spans = vec![
            Span::styled(pad(&mv.name, name_w), theme::value()),
            Span::styled(
                format!(" {:>2}/{:<2}", mv.pp, mv.pp_max),
                pp_style(mv),
            ),
            Span::styled(
                format!(
                    " {:>MOVE_NUM_W$} {:>MOVE_NUM_W$}",
                    num(mv.power),
                    num(mv.accuracy)
                ),
                theme::value(),
            ),
        ];
        if ty_w > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                pad(&mv.type_name, ty_w),
                theme::type_style(&mv.type_name),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// A move out of PP is a move that cannot be used, so it reads as a fault.
fn pp_style(mv: &MoveSlot) -> Style {
    if mv.pp == 0 {
        theme::bad()
    } else if mv.pp * 4 <= mv.pp_max {
        Style::default().fg(hp_color(0.3))
    } else {
        theme::label()
    }
}

/// The detail view for one Pokémon: everything the game keeps to itself.
///
/// The heading names **whose** Pokémon this is. Leaving that to be inferred
/// from the picture is exactly the bug this replaced: during a battle the
/// picture and the numbers can legitimately be different Pokémon, so the block
/// has to say which one it is describing.
pub fn detail_lines<'a>(
    m: &Mon,
    opponent: bool,
    mods: Option<terminalgb_plugin_trainer::derived::StatMods>,
    width: usize,
) -> Vec<Line<'a>> {
    let frac = m.hp_fraction();
    let hp = m.hidden_power();
    // A wild Pokémon's "nickname" is its species name, and an opponent may
    // have none at all — an empty bold field followed by `(CHARMANDER)` reads
    // as a missing value rather than as an absent one.
    let (title, sub) = if m.nickname.is_empty() || m.nickname == m.name {
        (m.name.clone(), String::new())
    } else {
        (m.nickname.clone(), m.name.clone())
    };
    let title_style = if opponent {
        theme::foe()
    } else {
        theme::selected()
    };
    let mut lines = vec![section(
        if opponent { "FOE" } else { "POKéMON" },
        Some(Span::styled(format!("#{:03}", m.dex), theme::label())),
        width,
    )];

    // Identity: the name, the species it really is, the level — one row.
    let mut ident = vec![Span::styled(
        clip(&title, width.saturating_sub(6)),
        title_style,
    )];
    let mut used = line_span_width(&ident);
    let level = format!("L{}", m.level);
    if !sub.is_empty() && used + 1 + sub.chars().count() + 1 + level.chars().count() <= width
    {
        ident.push(Span::styled(format!(" {sub}"), theme::label()));
        used += 1 + sub.chars().count();
    }
    if used + 1 + level.chars().count() <= width {
        let gap = width - used - level.chars().count();
        ident.push(Span::raw(" ".repeat(gap)));
        ident.push(Span::styled(level, theme::value()));
    }
    lines.push(Line::from(ident));

    // Types, coloured, on their own row so a dual type is never abbreviated
    // here — this is the page a reader opened to see it spelled out.
    lines.push(Line::from(type_spans(m, width.min(20))));

    // HP: bar, numbers, status.
    let bar = 12.min(width.saturating_sub(14));
    let mut hp_row = vec![
        Span::styled("HP ", theme::label()),
        Span::styled(hp_bar(frac, bar), Style::default().fg(hp_color(frac))),
        Span::styled(
            format!(" {}/{}", m.hp, m.max_hp),
            Style::default().fg(hp_color(frac)),
        ),
    ];
    if let Some(s) = m.status.label().or(if m.is_fainted() {
        Some("FNT")
    } else {
        None
    }) {
        if line_span_width(&hp_row) + 1 + 3 <= width {
            hp_row.push(Span::styled(format!(" {s}"), theme::status_style(s)));
        }
    }
    lines.push(Line::from(hp_row));

    lines.extend(stat_table(m, width));
    if let Some(l) = stat_mod_line(mods.or(m.stat_mods), "MOD", width) {
        lines.push(l);
    }
    lines.extend(move_table(&m.moves, width));
    lines.push(clamp_line(
        Line::from(vec![
            Span::styled("HIDDEN POWER ", theme::label()),
            Span::styled(hp.type_name, theme::type_style(hp.type_name)),
            Span::styled(format!(" {}", hp.power), theme::value()),
        ]),
        width,
    ));
    lines.push(Line::from(if m.would_be_shiny() {
        Span::styled("★ would be SHINY in Gen 2", theme::rare())
    } else {
        Span::styled("  not shiny if traded to Gen 2", theme::label())
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
    theme::shout()
}

/// The ordinary-encounter row, which must be readable without competing with
/// the game.
pub fn plain_banner_style() -> Style {
    theme::label()
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
/// [`terminalgb_plugin_trainer::derived::StatMods`] for why stages must never be
/// shown from stored data.
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
    let mut spans = vec![Span::styled(head, theme::label())];
    for (name, stage) in changed {
        let text = format!("{name}{stage:+}  ");
        if used + text.chars().count() > width {
            break;
        }
        used += text.chars().count();
        spans.push(Span::styled(text, theme::stage_style(stage)));
    }
    Some(Line::from(spans))
}

/// The opponent block — its own `FOE` section rather than an appended
/// afterthought, because a battle is the moment the panel matters most.
///
/// It is drawn **above** the party (see [`body_lines`]) and directly under the
/// strip that shows the other side, so their picture and their numbers are
/// adjacent. Nothing about the opponent is behind a key.
///
/// The stats stay on **one line** here rather than becoming the full
/// [`stat_table`]: a battle block is already a dozen rows, and the party is
/// what must not fall off the bottom. The table is one `Tab` and one `d` away.
pub fn opponent_lines<'a>(
    state: &TrainerState,
    focused: bool,
    width: usize,
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
    let frac = o.hp_fraction();
    let mut lines = vec![section(
        "FOE",
        Some(Span::styled(clip(&kind, width / 2), theme::label())),
        width,
    )];

    let c = party_cols(width);
    let nick = (c.nick + 2).min(width.saturating_sub(1));
    let mut head = vec![
        Span::styled(if focused { "▸" } else { " " }, theme::label()),
        Span::styled(pad(&o.name, nick), theme::foe()),
    ];
    let mut used = 1 + nick;
    if used + 5 <= width {
        head.push(Span::styled(" L", theme::label()));
        head.push(Span::styled(format!("{:>3}", o.level), theme::value()));
        used += 5;
    }
    if c.bar > 0 && used + 1 + c.bar <= width {
        head.push(Span::raw(" "));
        head.push(Span::styled(
            hp_bar(frac, c.bar),
            Style::default().fg(hp_color(frac)),
        ));
        used += 1 + c.bar;
    }
    if used + HP_NUMBERS <= width {
        head.push(Span::styled(
            format!(" {:>3}/{:>3}", o.hp, o.max_hp),
            Style::default().fg(hp_color(frac)),
        ));
        used += HP_NUMBERS;
    }
    if let Some(s) = o.status.label() {
        if used + 4 <= width {
            head.push(Span::styled(format!(" {s:<3}"), theme::status_style(s)));
            used += 4;
        }
    }
    if used + 4 <= width {
        head.push(Span::raw(" "));
        head.extend(type_spans(o, (width - used - 1).min(12)));
    }
    lines.push(clamp_line(Line::from(head), width));

    // One line of stats and one of DVs: the table is behind `Tab`+`d`, and the
    // party below must not be pushed off for it.
    lines.push(clamp_line(
        Line::from(vec![
            Span::styled("stat ", theme::label()),
            Span::styled(
                format!(
                    "{:>4}{:>5}{:>5}{:>5}",
                    o.attack, o.defense, o.speed, o.special
                ),
                theme::value(),
            ),
            Span::styled("  atk def spd spc", theme::label()),
        ]),
        width,
    ));
    if let Some(l) = stat_mod_line(o.stat_mods, "MOD", width) {
        lines.push(l);
    }
    // The DVs of whatever is on the other side, always — they are the numbers
    // behind the encounter banner's verdict, and a trainer's Pokémon showing
    // the fixed `$98/$88` pair every single time is itself worth being able to
    // see.
    let dvs = format!(
        "{:>4}{:>5}{:>5}{:>5}{:>5}",
        o.dvs.hp(),
        o.dvs.attack,
        o.dvs.defense,
        o.dvs.speed,
        o.dvs.special
    );
    let mut dv_row = vec![
        Span::styled("DV   ", theme::label()),
        Span::styled(
            dvs.clone(),
            if o.would_be_shiny() {
                theme::rare()
            } else {
                theme::value()
            },
        ),
    ];
    // The legend is the part that goes when the panel is narrow — a truncated
    // `hp/atk/def/sp` is worse than none, and the numbers are the point.
    let legend = "  hp atk def spd spc";
    if 5 + dvs.chars().count() + legend.chars().count() <= width {
        dv_row.push(Span::styled(legend, theme::label()));
    }
    lines.push(clamp_line(Line::from(dv_row), width));
    if state.battle == BattleKind::Wild && o.would_be_shiny() {
        lines.push(Line::from(Span::styled(
            "★ would be SHINY if traded to Gen 2",
            shiny_banner_style(),
        )));
    }

    // Moves, tabulated — an Ironmon run reads these before it reads anything
    // else on the panel, and a name with a PP count is not enough to choose
    // with. Only the slots that exist: an empty row in a battle block is a row
    // the party could have had.
    if o.moves.is_empty() {
        // A freshly-encountered Pokémon whose moves have not loaded yet.
        lines.push(Line::from(Span::styled(
            format!("moves {UNKNOWN}"),
            theme::label(),
        )));
    } else {
        lines.extend(move_table(&o.moves, width).into_iter().take(1 + o.moves.len()));
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
/// [`terminalgb_plugin_trainer::capture`], including why a Great Ball and an
/// Ultra Ball trade places depending on the target's HP.
pub fn catch_lines<'a>(state: &TrainerState, width: usize) -> Vec<Line<'a>> {
    let odds = state.catch_odds();
    if odds.is_empty() || !state.catchable() {
        return Vec::new();
    }
    let arrow = match state.catch_delta {
        Some(d) if d > 0.0005 => Some(("▲", theme::good())),
        Some(d) if d < -0.0005 => Some(("▼", theme::bad())),
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
        theme::good().add_modifier(Modifier::BOLD),
    )];
    if let Some((glyph, style)) = arrow {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(glyph, style));
    }
    let mut lines = vec![Line::from(spans)];
    // The one thing worth spelling out: below a quarter of maximum HP the HP
    // term saturates and no further chip damage helps at all. A player who does
    // not know that keeps whittling for nothing.
    if best.hp_maxed && width >= 30 {
        lines.push(Line::from(Span::styled(
            "HP is as low as it counts — status next",
            theme::label(),
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
        return Line::from(Span::styled(clip(&why, width), theme::label()));
    }
    let standing = state
        .enemy_team
        .iter()
        .filter(|m| m.known && !m.is_fainted())
        .count();
    let mut spans = Vec::new();
    let mut used = 0usize;
    if label {
        spans.push(Span::styled("TEAM ", theme::label()));
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
            (UNKNOWN, theme::label())
        } else if m.is_fainted() {
            ("○", theme::label())
        } else {
            ("●", theme::bad())
        };
        spans.push(Span::styled(glyph, style));
        used += 1;
    }
    let tail = format!("  {standing} left");
    if used + tail.chars().count() <= width {
        spans.push(Span::styled(tail, theme::label()));
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
                        theme::label()
                    } else {
                        theme::value()
                    },
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// The badges held, by name — the one thing the two-row header cannot say,
/// kept for the detail view's footer where there is room for it.
pub fn badge_names<'a>(state: &TrainerState, width: usize) -> Vec<Line<'a>> {
    let flags = state.badge_flags();
    let got: Vec<&str> = BADGE_NAMES
        .iter()
        .zip(flags)
        .filter(|(_, g)| *g)
        .map(|(n, _)| *n)
        .collect();
    if got.is_empty() {
        return Vec::new();
    }
    vec![clamp_line(
        Line::from(vec![
            Span::styled("BADGES ", theme::label()),
            Span::styled(got.join(", "), theme::value()),
        ]),
        width,
    )]
}

/// Everything below the strips, assembled in the order the situation calls for.
///
/// **Out of battle** the party list is the panel. **In battle** the opponent
/// goes first — directly under the strip showing the other side — and the party
/// follows it, still one line each, so all six are on screen. Nothing is hidden
/// behind a key and nothing scrolls.
pub fn body_lines<'a>(
    state: &TrainerState,
    focus: Option<Focus>,
    detail: bool,
    width: usize,
) -> Vec<Line<'a>> {
    body_rows(state, focus, detail, width)
        .into_iter()
        .map(|l| clamp_line(l, width))
        .collect()
}

fn body_rows<'a>(
    state: &TrainerState,
    focus: Option<Focus>,
    detail: bool,
    width: usize,
) -> Vec<Line<'a>> {
    if detail {
        let Some((f, m)) = focus.and_then(|f| focused_mon(state, f).map(|m| (f, m)))
        else {
            return vec![Line::from(Span::styled("  no Pokémon yet", theme::label()))];
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
        let mut lines = detail_lines(m, f.is_opponent(), mods, width);
        // The roster follows the opponent's own numbers even here, because
        // "what else has this trainer got" is part of the same decision.
        if f.is_opponent() && state.battle == BattleKind::Trainer {
            let l = team_line(state, width, true);
            if line_width(&l) > 0 {
                lines.push(separator(width));
                lines.push(l);
                lines.extend(team_names(state, width));
            }
        } else if !f.is_opponent() {
            let names = badge_names(state, width);
            if !names.is_empty() {
                lines.push(separator(width));
                lines.extend(names);
            }
        }
        return lines;
    }
    let fighting = state.opponent.is_some() && state.battle != BattleKind::None;
    let selected = match focus {
        Some(Focus::Party(i)) => Some(i),
        _ => None,
    };
    let mut lines = Vec::new();
    if fighting {
        lines.extend(opponent_lines(state, focus == Some(Focus::Opponent), width));
    }
    lines.extend(party_lines(state, selected, width));
    lines
}

/// Which party slot the battle engine currently has out, by matching the
/// in-battle copy back to the party. Gen 1 keeps no index we can read directly;
/// nickname *and* species together are unambiguous in practice, and a miss just
/// means no slot is marked.
pub fn active_slot(state: &TrainerState) -> Option<usize> {
    let a = state.active.as_ref()?;
    // Nickname *and* species is unambiguous when both are readable. But
    // `wBattleMonNick` is filled a little after `wBattleMon` and can be empty
    // or stale for the first frames of a battle, and a marker that appears a
    // beat late reads as a bug — so fall back to species and level, then to
    // species alone. A wrong guess here costs a marker, not a number.
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

/// A type label that fits `max` columns, as plain text — the styled form is
/// [`type_spans`], which is what the panel actually draws. Kept for the
/// abbreviation test, which is about the *text* and not the colour.
#[cfg(test)]
fn type_text(m: &Mon, max: usize) -> String {
    type_spans(m, max)
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

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

fn line_span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// The whole panel as a `Paragraph`, used by the message screens.
pub fn message<'a>(text: &'a str, title: &'a str) -> Paragraph<'a> {
    Paragraph::new(text)
        .wrap(ratatui::widgets::Wrap { trim: true })
        .style(theme::label())
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
                },
                MoveSlot {
                    id: 2,
                    name: "QUICK ATTACK".into(),
                    pp: 0,
                    pp_max: 30,
                    pp_ups: 0,
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
            },
            MoveSlot {
                id: 4,
                name: "EMBER".into(),
                pp: 25,
                pp_max: 25,
                pp_ups: 0,
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
                let l = layout(area, true);
                for a in [l.header, l.sprite, l.body] {
                    assert!(
                        a.x >= area.x
                            && a.y >= area.y
                            && a.right() <= area.right()
                            && a.bottom() <= area.bottom(),
                        "{a:?} escapes {area:?}"
                    );
                }
                assert!(
                    l.header.bottom() <= l.sprite.y,
                    "header runs into the sprite at {w}x{h}"
                );
                assert!(
                    l.sprite.bottom() <= l.body.y,
                    "the sprite would be drawn over at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn a_short_panel_drops_the_sprite_before_the_text() {
        // Border (2) + header (5) + the sprite band + six rows of text; below
        // that the picture goes and the numbers stay.
        let band = sprite_rows(PANEL_W - 2);
        let need = 2 + HEADER_ROWS + band + MIN_BODY_ROWS;
        let short = layout(Rect::new(0, 0, PANEL_W, need - 1), true);
        assert_eq!(short.sprite.height, 0);
        assert!(short.body.height > 0, "the text must survive");
        let tall = layout(Rect::new(0, 0, PANEL_W, need), true);
        assert_eq!(tall.sprite.height, band);
    }

    /// The sprite band is the panel's one genuinely scalable part, so it has to
    /// actually grow with the panel — and stay bounded at both ends.
    #[test]
    fn the_sprite_band_grows_with_the_panel() {
        assert!(sprite_rows(20) < sprite_rows(40));
        assert!(sprite_rows(40) < sprite_rows(60));
        assert_eq!(sprite_rows(4), 6, "a floor a Pokémon is still legible at");
        assert_eq!(sprite_rows(400), 16, "and a ceiling, so text still fits");
        for w in 0u16..200 {
            let r = sprite_rows(w);
            assert!((6..=16).contains(&r), "{w} -> {r}");
        }
    }

    /// …and so do the party columns: above the design width they grow rather
    /// than stranding the space, which is the only way text scales on a grid.
    #[test]
    fn the_party_columns_grow_with_a_wide_panel() {
        let narrow = party_cols(38);
        let wide = party_cols(60);
        assert!(wide.nick > narrow.nick && wide.bar > narrow.bar);
        // …but not without bound: past a point a longer bar is just a longer
        // bar, and the columns stay in proportion to one another.
        let huge = party_cols(200);
        assert_eq!((huge.nick, huge.bar, huge.ty), (18, 20, 16));
    }

    #[test]
    fn asking_for_no_sprite_leaves_no_gap() {
        let l = layout(Rect::new(0, 0, PANEL_W, 40), false);
        assert_eq!(l.sprite.height, 0);
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
    /// captain plays in — with the sprite still up.
    #[test]
    fn a_battle_still_shows_the_whole_party() {
        let mut st = battling();
        while st.party.len() < 6 {
            st.party.push(mon(30, 41));
        }
        let panel = Rect::new(0, 0, PANEL_W, 30);
        // The width does not depend on the pictures, so the body can be
        // composed first and the layout told what it needs.
        let width = panel.width.saturating_sub(2) as usize;
        let lines = body_lines(&st, Some(Focus::Opponent), false, width);
        let l = layout_with_team(panel, true, true, lines.len() as u16);
        assert!(
            lines.len() <= l.body.height as usize,
            "a battle overflows the body at 40x30: {} lines into {} rows",
            lines.len(),
            l.body.height
        );
        // …and the picture is what gave way for it, not the party.
        assert_eq!(l.sprite.height, 0, "the sprite is the first thing to lose");
        let text = plain(&lines).join("\n");
        assert!(
            text.contains("OPPONENT"),
            "the opponent has its own section"
        );
        assert!(text.contains("CHARMANDER"));
        assert!(text.contains("SCRATCH"), "and its moves are on screen");
        // Every party member is still listed.
        for m in &st.party {
            assert!(
                text.contains(&m.nickname.chars().take(6).collect::<String>()),
                "party member {} fell off the panel",
                m.nickname
            );
        }
    }

    /// The opponent comes before the party during a battle: it is what is being
    /// watched, and it belongs next to the sprite that has switched to it.
    #[test]
    fn the_opponent_section_is_above_the_party() {
        let st = battling();
        let text = plain(&body_lines(&st, Some(Focus::Opponent), false, 38)).join("\n");
        let o = text.find("OPPONENT").expect("an opponent section");
        let p = text.find("[ PARTY ]").expect("a party section");
        assert!(o < p, "the opponent must come first in a battle");
    }

    #[test]
    fn every_section_is_announced_by_a_bracketed_rule() {
        let st = filled_state();
        let head = plain(&header_lines(&st, "Pokémon Blue", 38)).join("\n");
        assert!(head.contains("[ TRAINER ]"));
        assert!(head.contains("[ BADGES ]"));
        let body = plain(&body_lines(&st, Some(Focus::Party(0)), false, 38)).join("\n");
        assert!(body.contains("[ PARTY ]"));
        let detail = plain(&body_lines(&st, Some(Focus::Party(0)), true, 38)).join("\n");
        assert!(detail.contains("[ YOUR POKéMON ]"));
        assert!(detail.contains("[ MOVES ]"));
    }

    #[test]
    fn a_rule_is_centred_and_exactly_the_width_asked_for() {
        // A rule always fills exactly the width it is given, down to the
        // point where the label itself no longer fits and gets clipped.
        for w in 9usize..=60 {
            assert_eq!(line_width(&rule("PARTY", w)), w, "at width {w}");
        }
        for w in 0usize..9 {
            assert!(line_width(&rule("PARTY", w)) <= w, "at width {w}");
        }
        assert_eq!(plain(&[rule("PARTY", 21)])[0], "──────[ PARTY ]──────");
        assert_eq!(plain(&[rule("PARTY", 9)])[0], "[ PARTY ]");
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
        assert!(text.contains("] ["), "two groups of four");
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

    #[test]
    fn the_badge_row_degrades_rather_than_wrapping() {
        let st = TrainerState {
            badges: 0xFF,
            ..Default::default()
        };
        assert!(plain(&[badge_row(&st, 34)])[0].contains("[●]"));
        let mid = plain(&[badge_row(&st, 20)])[0].clone();
        assert!(!mid.contains("[●]"), "brackets go first: {mid}");
        assert!(mid.contains("●●●●"), "{mid}");
        let tiny = plain(&[badge_row(&st, 5)])[0].clone();
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
        let wide = plain(&header_lines(&st, "Pokémon Blue", 38))[1].clone();
        assert!(wide.contains("ID"), "{wide}");
        assert!(wide.contains("3,000"), "{wide}");
        let narrow = plain(&header_lines(&st, "Pokémon Blue", 20))[1].clone();
        assert!(!narrow.contains("ID"), "the ID goes first: {narrow}");
        assert!(narrow.contains("3,000"), "the money stays: {narrow}");
        let tiny = plain(&header_lines(&st, "Pokémon Blue", 11))[1].clone();
        assert!(tiny.contains("BLUE"), "the name never goes: {tiny}");
    }

    #[test]
    fn a_long_type_is_abbreviated_rather_than_truncated() {
        let m = mon(20, 41);
        assert_eq!(type_text(&m, 12), "GRASS/POISON");
        assert_eq!(type_text(&m, 8), "GRA/POI");
        let single = Mon {
            types: ("ELECTRIC".into(), "ELECTRIC".into()),
            ..mon(20, 41)
        };
        assert_eq!(type_text(&single, 8), "ELECTRIC");
        assert_eq!(type_text(&single, 4), "ELE");
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

    /// The strip is the *last* thing granted space: it may never be the reason
    /// the opponent's stats or the party fall off the bottom.
    #[test]
    fn the_team_strip_never_squeezes_out_the_party() {
        for h in 12u16..=48 {
            for w in [24u16, 30, 40, 56] {
                let a = Rect::new(0, 0, w, h);
                let l = layout_with_team(a, true, true, MIN_BODY_ROWS);
                let plain = layout_with_team(a, true, false, MIN_BODY_ROWS);
                assert!(
                    l.body.height >= MIN_BODY_ROWS || l.team.height == 0,
                    "{w}x{h}: the strip left only {} body rows",
                    l.body.height
                );
                assert!(
                    l.body.height == plain.body.height || l.team.height > 0,
                    "{w}x{h}: refusing the strip must cost the body nothing"
                );
                assert!(l.sprite.bottom() <= l.team.y);
                assert!(l.team.bottom() <= l.body.y);
                assert!(l.team.bottom() <= a.bottom());
                if l.team.height > 0 {
                    assert!(w >= TEAM_MIN_W, "{w}x{h}: strip drawn too narrow");
                }
            }
        }
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
        assert!(
            text.contains("BADGES"),
            "the summary is there out of battle"
        );
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

    /// The panel is dense and the captain splits it narrow. The row must lose
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

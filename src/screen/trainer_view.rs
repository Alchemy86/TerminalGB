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
//! One bordered frame holding **bracketed sections** separated by centred rules
//! — `[ TRAINER ]`, `[ BADGES ]`, `[ PARTY ]`, `[ OPPONENT ]`. The rules cost
//! one row each and are what stops the panel reading as an undifferentiated
//! column of text; everything inside a section is aligned to fixed columns so a
//! value changing never shifts the one beside it.
//!
//! ```text
//! ┌ Trainer · d detail ──────────────────┐
//! │─────────── [ TRAINER ] ──────────────│
//! │NAME BLUE │ ID 62800 │ ¥3,000         │
//! │TIME 00:01:41 │ Pokémon Blue          │
//! │──────────── [ BADGES ] ──────────────│
//! │[●][●][○][○] [○][○][○][○]  2/8        │
//! │                                      │
//! │            ( the sprite )            │
//! │                                      │
//! │           ● ● ● ○ ○ ○                │
//! │──────────── [ PARTY ] ───────────────│
//! │▸1 SPARK       L 12 ████░░░░ PSN ★    │
//! │   PIKACHU      34/ 34  ELECTRIC      │
//! │──────────────────────────────────────│
//! │BADGES Boulder, Cascade               │
//! │NEXT   Lt.Surge                       │
//! └──────────────────────────────────────┘
//! ```
//!
//! # Degrading as the panel narrows
//!
//! Every section takes the width it is given and drops its **least valuable
//! column first** rather than wrapping into mush. The party rows have four
//! tiers ([`party_cols`]); the trainer fields drop by a declared priority
//! ([`Field::drop`]); the badge row falls from `[●][●]…` to `●●…` to a bare
//! count. `no_line_overflows_at_any_width` renders every section at every width
//! from 8 to 60 and asserts nothing exceeds it — the layout is guaranteed by
//! that test, not by arithmetic in a comment.
//!
//! # Glyphs
//!
//! Only characters already used elsewhere in this frontend: box drawing
//! (`─ │ ┌`), the block elements `█ ▌ ░`, the geometric shapes `● ○ ▸`, and `★`.
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
//! stat experience, and the derived values in [`crate::trainer::derived`].

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::trainer::encounter::WildEncounter;
use crate::trainer::gen1::{BattleKind, BattleType};
use crate::trainer::gen1::{Mon, BADGE_LEADERS, BADGE_NAMES};
use crate::trainer::{TeamMon, TrainerState};

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

/// Rows reserved for the sprite. A Gen 1 picture is at most 56x56 pixels, and
/// terminal cells are roughly twice as tall as wide, so eight rows is about
/// square at any sane font.
pub const SPRITE_ROWS: u16 = 8;

/// Rows the trainer header always takes: two section rules, two field lines and
/// the badge row.
pub const HEADER_ROWS: u16 = 5;

/// Rows of text that must survive before the sprite is allowed any. The party
/// is the point; the picture is the decoration.
const MIN_BODY_ROWS: u16 = 6;

/// Rows the opposing trainer's team strip takes when it is drawn as pictures.
/// Three rows of a 6:1 strip is about the smallest a Pokémon stays recognisable
/// at, and it is what the panel can spare.
pub const TEAM_ROWS: u16 = 3;

/// Narrowest panel that gets the team strip as pictures rather than as markers.
/// Below this each Pokémon would be under four cells wide, which is a smudge.
pub const TEAM_MIN_W: u16 = 30;

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Where the panel's pieces go inside its rectangle.
///
/// Everything except [`frame`](PanelLayout::frame) is *inside* the border, so
/// the panel reads as one object rather than as two stacked boxes.
pub struct PanelLayout {
    /// The single bordered block, drawn over the whole panel rectangle.
    pub frame: Rect,
    /// The trainer header: name, ID, money, play time, badges.
    pub header: Rect,
    /// The picture of the focused Pokémon. Empty (`height == 0`) when there is
    /// no room for it — the text must still be readable on a short terminal.
    pub sprite: Rect,
    /// The opposing trainer's team, as a single wide strip image. Empty
    /// (`height == 0`) for a wild encounter, on a narrow or short panel, or
    /// when the roster did not read back — in which case the `[ OPPONENT ]`
    /// section carries the markers instead.
    pub team: Rect,
    /// The party list, or the detail view.
    pub body: Rect,
}

/// Split the panel rectangle. Never returns overlapping rectangles, and never
/// returns a sprite area that the text would be drawn over.
pub fn layout(area: Rect, want_sprite: bool) -> PanelLayout {
    layout_with_team(area, want_sprite, false, MIN_BODY_ROWS)
}

/// [`layout`] plus the opposing trainer's team strip.
///
/// `body_rows` is how many rows of text the body actually wants — the caller
/// has already composed them, so it costs nothing to ask. **Both pictures are
/// granted space only if the text still fits after them.** That is what stops a
/// battle, whose `[ OPPONENT ]` block is a dozen rows on its own, from pushing
/// the party off the bottom: the picture is the decoration and it goes first,
/// then the team strip, and the numbers stay.
///
/// The strip is **progressive and the last thing in**: it also needs at least
/// [`TEAM_MIN_W`] columns. Where it does not fit, the roster is still reported
/// — as markers on a single line inside `[ OPPONENT ]`.
pub fn layout_with_team(
    area: Rect,
    want_sprite: bool,
    want_team: bool,
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
    // The sprite is the first thing to lose when the terminal is short.
    let need = body_rows.max(MIN_BODY_ROWS);
    let sprite_h = if want_sprite && rest_h >= SPRITE_ROWS + need {
        SPRITE_ROWS
    } else {
        0
    };
    let after_sprite = rest_h.saturating_sub(sprite_h);
    let team_h =
        if want_team && inner.width >= TEAM_MIN_W && after_sprite >= TEAM_ROWS + need {
            TEAM_ROWS
        } else {
            0
        };
    PanelLayout {
        frame: area,
        header: Rect::new(inner.x, inner.y, inner.width, header_h),
        sprite: Rect::new(inner.x, inner.y + header_h, inner.width, sprite_h),
        team: Rect::new(inner.x, inner.y + header_h + sprite_h, inner.width, team_h),
        body: Rect::new(
            inner.x,
            inner.y + header_h + sprite_h + team_h,
            inner.width,
            after_sprite.saturating_sub(team_h),
        ),
    }
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

/// A centred section rule: `───── [ PARTY ] ─────`.
///
/// The bracketed title is what gives the panel its structure; when there is not
/// room for the rule the title still survives, because losing the label is
/// worse than losing the decoration.
pub fn rule<'a>(title: &str, width: usize) -> Line<'a> {
    let deco = format!("[ {title} ]");
    let n = deco.chars().count();
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    if width < n {
        // Narrower than the label: keep as much of the label as fits.
        return Line::from(Span::styled(clip(&deco, width), title_style));
    }
    let left = (width - n) / 2;
    let right = width - n - left;
    Line::from(vec![
        Span::styled("─".repeat(left), dim()),
        Span::styled(deco, title_style),
        Span::styled("─".repeat(right), dim()),
    ])
}

/// A plain full-width separator, used before the summary lines so they read as
/// a footnote rather than as more party.
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
            spans.push(Span::styled(" │ ", dim()));
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
        spans.push(Span::raw(value));
    }
    Line::from(spans)
}

/// HP colour by remaining fraction. Three bands, because more than three is
/// noise at a glance.
pub fn hp_color(fraction: f32) -> Color {
    if fraction <= 0.2 {
        Color::Red
    } else if fraction <= 0.5 {
        Color::Yellow
    } else {
        Color::Green
    }
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

/// The trainer header: two field lines and the badge row, under their rules.
///
/// Exactly [`HEADER_ROWS`] lines whatever the width, so the sprite below it
/// never moves.
pub fn header_lines<'a>(state: &TrainerState, game: &str, width: usize) -> Vec<Line<'a>> {
    header_rows(state, game, width)
        .into_iter()
        .map(|l| clamp_line(l, width))
        .collect()
}

fn header_rows<'a>(state: &TrainerState, game: &str, width: usize) -> Vec<Line<'a>> {
    let (h, m, s) = state.play_time;
    vec![
        rule("TRAINER", width),
        fields_line(
            vec![
                Field {
                    label: "NAME",
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
                    value: format!("¥{}", thousands(state.money)),
                    drop: 1,
                },
            ],
            width,
        ),
        fields_line(
            vec![
                Field {
                    label: "TIME",
                    value: format!("{h:02}:{m:02}:{s:02}"),
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
        rule("BADGES", width),
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

    // `[●][●][●][●] [●][●][●][●]  n/8`
    let bracketed = 8 * 3 + 1 + 2 + count.chars().count();
    // `●●●● ●●●●  n/8`
    let bare = 8 + 1 + 2 + count.chars().count();

    let mut spans = Vec::new();
    if width >= bracketed {
        for (i, &on) in flags.iter().enumerate() {
            if i == 4 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled("[", dim()));
            spans.push(if on {
                Span::styled("●", got)
            } else {
                Span::styled("○", dim())
            });
            spans.push(Span::styled("]", dim()));
        }
    } else if width >= bare {
        for (i, &on) in flags.iter().enumerate() {
            if i == 4 {
                spans.push(Span::raw(" "));
            }
            spans.push(if on {
                Span::styled("●", got)
            } else {
                Span::styled("○", dim())
            });
        }
    } else {
        // No room for markers at all: the count is the part that cannot go.
        return Line::from(Span::styled(clip(&count, width), got));
    }
    spans.push(Span::styled(format!("  {count}"), dim()));
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
    let n = crate::trainer::gen1::PARTY_LENGTH;
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
    if width >= 34 {
        PartyCols {
            nick: 10,
            bar: 8,
            status: true,
            ty: 12,
        }
    } else if width >= 28 {
        PartyCols {
            nick: 8,
            bar: 5,
            status: true,
            ty: 8,
        }
    } else if width >= 22 {
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
    // fight. Focus wins the cell, because the slot row above underlines the
    // active one anyway.
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
                Style::default().fg(Color::Cyan)
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
    spans.push(Span::raw(format!(" L{:>3}", m.level)));
    used += 4;
    if c.bar > 0 && used + 1 + c.bar <= width {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            hp_bar(frac, c.bar),
            Style::default().fg(hp_color(frac)),
        ));
        used += 1 + c.bar;
    }
    if c.status && used + 4 <= width {
        let label = m
            .status
            .label()
            .unwrap_or(if m.is_fainted() { "FNT" } else { "" });
        spans.push(Span::styled(
            format!(" {label:<3}"),
            Style::default().fg(Color::Magenta),
        ));
        used += 4;
    }
    if m.would_be_shiny() && used < width {
        spans.push(Span::styled("★", Style::default().fg(Color::LightYellow)));
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
        spans.push(Span::styled(
            format!(" {}", type_text(m, room)),
            Style::default().fg(Color::Cyan),
        ));
    }
    Line::from(spans)
}

/// The party list. Two lines per Pokémon; `compact` collapses that to one,
/// which is what a battle uses so the opponent block cannot push the team off
/// the bottom of the panel.
pub fn party_lines<'a>(
    state: &TrainerState,
    selected: Option<usize>,
    width: usize,
    compact: bool,
) -> Vec<Line<'a>> {
    if state.party.is_empty() {
        return vec![Line::from(Span::styled("  no Pokémon yet", dim()))];
    }
    let c = party_cols(width);
    let active = active_slot(state);
    let mut lines = Vec::new();
    for (i, m) in state.party.iter().enumerate() {
        lines.push(party_head(
            i,
            m,
            selected == Some(i),
            active == Some(i),
            c,
            width,
        ));
        if !compact {
            lines.push(party_body(m, c, width));
        }
    }
    lines
}

/// The detail view for one Pokémon: everything the game keeps to itself.
///
/// The heading names **whose** Pokémon this is. Leaving that to be inferred
/// from the picture is exactly the bug this replaced: during a battle the
/// sprite and the numbers can legitimately be different Pokémon, so the block
/// has to say which one it is describing.
pub fn detail_lines<'a>(
    m: &Mon,
    opponent: bool,
    mods: Option<crate::trainer::derived::StatMods>,
    width: usize,
) -> Vec<Line<'a>> {
    let frac = m.hp_fraction();
    let hp = m.hidden_power();
    let bar = 12.min(width.saturating_sub(14));
    // A wild Pokémon's "nickname" is its species name, and an opponent may
    // have none at all — an empty bold field followed by `(CHARMANDER)` reads
    // as a missing value rather than as an absent one.
    let (title, sub) = if m.nickname.is_empty() || m.nickname == m.name {
        (m.name.clone(), String::new())
    } else {
        (m.nickname.clone(), format!("({})", m.name))
    };
    let mut lines = vec![
        rule(
            if opponent {
                "OPPONENT"
            } else {
                "YOUR POKéMON"
            },
            width,
        ),
        Line::from(vec![
            Span::styled(
                format!("{} ", clip(&title, width.saturating_sub(4))),
                Style::default()
                    .fg(if opponent { Color::Red } else { Color::Reset })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sub, dim()),
        ]),
        Line::from(vec![
            Span::raw(format!("#{:03}  L{:<4}", m.dex, m.level)),
            Span::styled(m.type_label(), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("HP  "),
            Span::styled(hp_bar(frac, bar), Style::default().fg(hp_color(frac))),
            Span::raw(format!(" {}/{}", m.hp, m.max_hp)),
        ]),
        stat_line(m.attack, m.defense, m.speed, m.special, width),
    ];
    if let Some(l) = stat_mod_line(mods.or(m.stat_mods), "MOD", width) {
        lines.push(l);
    }
    lines.push(rule("MOVES", width));
    for i in 0..4 {
        match m.moves.get(i) {
            Some(mv) => lines.push(Line::from(vec![
                Span::raw(pad(&mv.name, width.saturating_sub(9).min(14))),
                Span::styled(
                    format!(" {:>2}/{:<2}", mv.pp, mv.pp_max),
                    Style::default().fg(if mv.pp == 0 {
                        Color::Red
                    } else {
                        Color::Reset
                    }),
                ),
                Span::styled(
                    if mv.pp_ups > 0 {
                        format!(" +{}", mv.pp_ups)
                    } else {
                        String::new()
                    },
                    dim(),
                ),
            ])),
            // An empty move slot is genuinely empty, not unknown — so it gets a
            // dash rather than the unknown glyph.
            None => lines.push(Line::from(Span::styled("—", dim()))),
        }
    }
    lines.push(rule("HIDDEN", width));
    lines.push(Line::from(Span::raw(format!(
        "DV {:>2}/{:>2}/{:>2}/{:>2}/{:>2}  {}/75",
        m.dvs.hp(),
        m.dvs.attack,
        m.dvs.defense,
        m.dvs.speed,
        m.dvs.special,
        m.dv_total()
    ))));
    // Stat experience runs to five digits, so five columns of it plus a label
    // is 33 — one more than a 34-column panel has inside it. The label is what
    // gives, not a digit; the column header is derived from the same label so
    // the two cannot drift out of alignment.
    let label = if width >= 33 { "EXP " } else { "E " };
    lines.push(Line::from(Span::styled(
        format!(
            "{}{:>5} {:>5} {:>5} {:>5} {:>5}",
            " ".repeat(label.len()),
            "hp",
            "atk",
            "def",
            "spd",
            "spc"
        ),
        dim(),
    )));
    match &m.stat_exp {
        Some(e) => {
            lines.push(Line::from(Span::raw(format!(
                "{label}{:>5} {:>5} {:>5} {:>5} {:>5}",
                e.hp, e.attack, e.defense, e.speed, e.special
            ))));
        }
        // The in-battle structure does not carry stat experience, so during a
        // battle this is unknown rather than zero.
        None => lines.push(Line::from(vec![
            Span::raw("EXP "),
            Span::styled(
                clip(
                    &format!("{UNKNOWN} (not in the battle copy)"),
                    width.saturating_sub(4),
                ),
                dim(),
            ),
        ])),
    }
    lines.push(Line::from(vec![
        Span::styled("HIDDEN POWER ", dim()),
        Span::styled(
            format!("{} {}", hp.type_name, hp.power),
            Style::default().fg(Color::Cyan),
        ),
    ]));
    lines.push(Line::from(if m.would_be_shiny() {
        Span::styled(
            "★ would be SHINY in Gen 2",
            Style::default()
                .fg(Color::LightYellow)
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
/// [`crate::trainer::derived::StatMods`] for why stages must never be shown
/// from stored data.
pub fn stat_mod_line<'a>(
    mods: Option<crate::trainer::derived::StatMods>,
    label: &str,
    width: usize,
) -> Option<Line<'a>> {
    let changed = mods?.changed();
    if changed.is_empty() {
        return None;
    }
    let head = format!("{label:<4}");
    let mut used = head.chars().count();
    let mut spans = vec![Span::styled(head, dim())];
    for (name, stage) in changed {
        let text = format!("{name}{stage:+}  ");
        if used + text.chars().count() > width {
            break;
        }
        used += text.chars().count();
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(if stage < 0 { Color::Red } else { Color::Green })
                .add_modifier(Modifier::BOLD),
        ));
    }
    Some(Line::from(spans))
}

/// The four battle stats on one line, dropping to two labels per line if the
/// panel cannot hold four.
fn stat_line<'a>(atk: u16, def: u16, spd: u16, spc: u16, width: usize) -> Line<'a> {
    let full = format!("ATK {atk:<4} DEF {def:<4} SPD {spd:<4} SPC {spc}");
    if full.chars().count() <= width {
        return Line::from(Span::raw(full));
    }
    Line::from(Span::raw(clip(
        &format!("A{atk} D{def} S{spd} C{spc}"),
        width,
    )))
}

/// The opponent section — its own `[ OPPONENT ]` block rather than an appended
/// afterthought, because a battle is the moment the panel matters most.
///
/// It is drawn **above** the party (see [`body_lines`]) so it sits next to the
/// sprite, which has already switched to the opponent; and the party below it
/// collapses to one line each, so the team is still on screen. Nothing about
/// the opponent is behind a key.
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
    let c = party_cols(width);
    let frac = o.hp_fraction();
    let mut lines = vec![rule("OPPONENT", width)];

    let nick = (c.nick + 2).min(width.saturating_sub(1));
    let mut head = vec![
        Span::styled(if focused { "▸" } else { " " }, dim()),
        Span::styled(
            pad(&o.name, nick),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    ];
    let mut used = 1 + nick;
    if used + 4 <= width {
        head.push(Span::raw(format!(" L{:>3}", o.level)));
        used += 4;
    }
    if c.bar > 0 && used + 1 + c.bar <= width {
        head.push(Span::raw(" "));
        head.push(Span::styled(
            hp_bar(frac, c.bar),
            Style::default().fg(hp_color(frac)),
        ));
        used += 1 + c.bar;
    }
    if let Some(s) = o.status.label() {
        if used + 4 <= width {
            head.push(Span::styled(
                format!(" {s:<3}"),
                Style::default().fg(Color::Magenta),
            ));
        }
    }
    lines.push(Line::from(head));

    let mut sub = vec![Span::styled(pad(&kind, nick + 1), dim())];
    let mut used = nick + 1;
    if used + 8 <= width {
        sub.push(Span::styled(format!(" {:>3}/{:>3}", o.hp, o.max_hp), dim()));
        used += 8;
        if used + 2 <= width {
            sub.push(Span::styled(
                format!(" {}", type_text(o, width - used - 1)),
                Style::default().fg(Color::Cyan),
            ));
        }
    }
    lines.push(Line::from(sub));
    lines.push(stat_line(o.attack, o.defense, o.speed, o.special, width));
    if let Some(l) = stat_mod_line(o.stat_mods, "MOD", width) {
        lines.push(l);
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
    let mut dv_row = vec![Span::styled("DV  ", dim()), Span::raw(dvs.clone())];
    // The legend is the part that goes when the panel is narrow — a truncated
    // `hp/atk/def/sp` is worse than none, and the numbers are the point.
    let legend = "  hp/atk/def/spd/spc";
    if 4 + dvs.chars().count() + legend.chars().count() <= width {
        dv_row.push(Span::styled(legend, dim()));
    }
    lines.push(Line::from(dv_row));
    if state.battle == BattleKind::Wild && o.would_be_shiny() {
        lines.push(Line::from(Span::styled(
            "★ would be SHINY if traded to Gen 2",
            shiny_banner_style(),
        )));
    }

    // Moves, two to a row while they fit — an Ironmon run reads these before it
    // reads anything else on the panel.
    if o.moves.is_empty() {
        // A freshly-encountered Pokémon whose moves have not loaded yet.
        lines.push(Line::from(Span::styled(format!("moves {UNKNOWN}"), dim())));
    } else {
        // Two to a row while they fit, in fixed-width cells with a gutter — a
        // pair of moves that runs together (`THUNDERBOLT 15/15QUICK ATTACK`)
        // reads as one long move with a strange PP count.
        let per = if width >= 34 { 2 } else { 1 };
        // A single-column row still keeps its name column tight: padding the
        // name to the whole panel strands the PP against the far edge, which
        // reads as two unrelated pieces of information.
        let cell = if per == 2 {
            (width - 1) / 2
        } else {
            width.min(20)
        };
        for chunk in o.moves.chunks(per) {
            let mut spans = Vec::new();
            for (i, mv) in chunk.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                let pp = format!("{:>2}/{:<2}", mv.pp, mv.pp_max);
                let name = pad(&mv.name, cell.saturating_sub(pp.chars().count() + 1));
                spans.push(Span::raw(name));
                spans.push(Span::styled(
                    format!(" {pp}"),
                    Style::default().fg(if mv.pp == 0 {
                        Color::Red
                    } else {
                        Color::DarkGray
                    }),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    // The roster line closes the section: a trainer battle is as much about
    // what is still in their belt as about what is on the field. When the strip
    // of pictures is drawn above, this is its caption; when it is not, this is
    // the whole report.
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

/// The summary footnote: which badges are in hand and whose gym is next.
///
/// Preceded by a plain separator rather than a bracketed rule, because it is a
/// footnote to the panel and not another section of it.
pub fn summary_lines<'a>(state: &TrainerState, width: usize) -> Vec<Line<'a>> {
    let flags = state.badge_flags();
    let got: Vec<&str> = BADGE_NAMES
        .iter()
        .zip(flags)
        .filter(|(_, g)| *g)
        .map(|(n, _)| *n)
        .collect();
    let next = BADGE_LEADERS
        .iter()
        .zip(flags)
        .find(|(_, g)| !*g)
        .map(|(n, _)| *n);
    vec![
        separator(width),
        Line::from(vec![
            Span::styled("BADGES ", dim()),
            Span::raw(clip(
                &if got.is_empty() {
                    "—".to_string()
                } else {
                    got.join(", ")
                },
                width.saturating_sub(7),
            )),
        ]),
        Line::from(vec![
            Span::styled("NEXT   ", dim()),
            Span::raw(clip(
                match next {
                    Some(leader) => leader,
                    None => "all eight",
                },
                width.saturating_sub(7),
            )),
        ]),
    ]
}

/// Everything below the sprite, assembled in the order the situation calls for.
///
/// **Out of battle** the party is the panel and the badge summary closes it.
/// **In battle** the opponent goes first — beside its own sprite, which the
/// frontend has already switched — the party collapses to one line each so all
/// six still fit, and the badge summary stands down. Nothing is hidden behind a
/// key and nothing scrolls: the *names* of the badges held are the one thing
/// that goes, and the count and the eight markers are still in the header two
/// rows up.
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
            return vec![Line::from(Span::styled("  no Pokémon yet", dim()))];
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
        }
        return lines;
    }
    let fighting = state.opponent.is_some() && state.battle != BattleKind::None;
    let selected = match focus {
        Some(Focus::Party(i)) => Some(i),
        _ => None,
    };
    let mut lines = vec![slot_row(state, active_slot(state), width)];
    if fighting {
        lines.extend(opponent_lines(state, focus == Some(Focus::Opponent), width));
    }
    lines.push(rule("PARTY", width));
    lines.extend(party_lines(state, selected, width, fighting));
    if !fighting {
        lines.extend(summary_lines(state, width));
    }
    lines
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
fn type_text(m: &Mon, max: usize) -> String {
    let full = m.type_label();
    if full.chars().count() <= max {
        return full;
    }
    let short = |t: &str| -> String { t.chars().take(3).collect() };
    let (a, b) = (&m.types.0, &m.types.1);
    let abbreviated = if b.is_empty() || a == b {
        short(a)
    } else {
        format!("{}/{}", short(a), short(b))
    };
    clip(&abbreviated, max)
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
    use crate::trainer::gen1::{Dvs, MoveSlot, Status};
    use crate::trainer::TeamMon;

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
        o.stat_mods = Some(crate::trainer::derived::StatMods::from_bytes([
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
        active.stat_mods = Some(crate::trainer::derived::StatMods::from_bytes([
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
        // Border (2) + header (5) + sprite (8) + six rows of text needs 21 rows;
        // below that the picture goes and the numbers stay.
        let short = layout(Rect::new(0, 0, PANEL_W, 20), true);
        assert_eq!(short.sprite.height, 0);
        assert!(short.body.height > 0, "the text must survive");
        let tall = layout(Rect::new(0, 0, PANEL_W, 21), true);
        assert_eq!(tall.sprite.height, SPRITE_ROWS);
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
            Some(crate::trainer::derived::StatMods::from_bytes([
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
            Some(crate::trainer::derived::StatMods::from_bytes([7; 6])),
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

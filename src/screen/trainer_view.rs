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

use crate::trainer::gen1::BattleKind;
use crate::trainer::gen1::{Mon, BADGE_LEADERS, BADGE_NAMES};
use crate::trainer::TrainerState;

/// Shown wherever a value is not known yet, so a blank never reads as a zero.
pub const UNKNOWN: &str = "?";

/// Width of the trainer panel in cells. Wide enough for a name, a level, a ten
/// cell HP bar and `cur/max`, which is the compact row.
pub const PANEL_W: u16 = 40;

/// Rows reserved for the sprite. A Gen 1 picture is at most 56x56 pixels, and
/// terminal cells are roughly twice as tall as wide, so eight rows is about
/// square at any sane font.
pub const SPRITE_ROWS: u16 = 8;

/// Where the panel's pieces go inside its rectangle.
pub struct PanelLayout {
    /// The trainer header: name, ID, money, play time, badges.
    pub header: Rect,
    /// The picture of the focused Pokémon. Empty (`height == 0`) when there is
    /// no room for it — the text must still be readable on a short terminal.
    pub sprite: Rect,
    /// The party list, or the detail view.
    pub body: Rect,
}

/// Split the panel rectangle. Never returns overlapping rectangles, and never
/// returns a sprite area that the text would be drawn over.
pub fn layout(area: Rect, want_sprite: bool) -> PanelLayout {
    // The header is fixed; the sprite is the first thing to lose when the
    // terminal is short, because the numbers are the point and the picture is
    // the decoration.
    const HEADER_H: u16 = 5;
    let header_h = HEADER_H.min(area.height);
    let rest_h = area.height.saturating_sub(header_h);
    // Keep at least four rows of text, so the sprite can never squeeze the
    // party list out entirely.
    let sprite_h = if want_sprite && rest_h >= SPRITE_ROWS + 4 {
        SPRITE_ROWS
    } else {
        0
    };
    PanelLayout {
        header: Rect::new(area.x, area.y, area.width, header_h),
        sprite: Rect::new(area.x, area.y + header_h, area.width, sprite_h),
        body: Rect::new(
            area.x,
            area.y + header_h + sprite_h,
            area.width,
            rest_h.saturating_sub(sprite_h),
        ),
    }
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
    let filled = if fraction > 0.0 { filled.max(1) } else { 0 };
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// The trainer header.
pub fn header<'a>(state: &TrainerState, game: &str) -> Paragraph<'a> {
    let flags = state.badge_flags();
    let badges: String = flags
        .iter()
        .map(|&got| if got { '●' } else { '○' })
        .collect();
    let (h, m, s) = state.play_time;

    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{:<8}", state.player_name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("ID {:05}", state.player_id),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(format!("  ¥{}", thousands(state.money))),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{h:02}:{m:02}:{s:02}"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!("  {game}"), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::raw(format!("Badges {}/8  ", state.badge_count())),
            Span::styled(badges, Style::default().fg(Color::Yellow)),
        ]),
    ];
    Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Trainer ")
            .border_style(Style::default().fg(Color::DarkGray)),
    )
}

/// The compact party list: one row per Pokémon, plus the badges each gym leader
/// still owes you when the party is short.
pub fn party_lines<'a>(state: &TrainerState, selected: usize) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    if state.party.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no Pokémon yet",
            Style::default().fg(Color::DarkGray),
        )));
        return lines;
    }
    for (i, m) in state.party.iter().enumerate() {
        let marker = if i == selected { "▸" } else { " " };
        let frac = m.hp_fraction();
        let status =
            m.status
                .label()
                .unwrap_or(if m.is_fainted() { "FNT" } else { "   " });
        let mut spans = vec![
            Span::styled(
                format!("{marker}{} ", i + 1),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<10}", truncate(&m.nickname, 10)),
                Style::default().add_modifier(if i == selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(
                format!("{status:<3} "),
                Style::default().fg(if status.trim().is_empty() {
                    Color::DarkGray
                } else {
                    Color::Magenta
                }),
            ),
            Span::raw(format!("L{:<3}", m.level)),
            Span::styled(hp_bar(frac, 8), Style::default().fg(hp_color(frac))),
        ];
        if m.would_be_shiny() {
            spans.push(Span::styled("★", Style::default().fg(Color::LightYellow)));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(Span::styled(
            format!(
                "    {:<10} {:>3}/{:<3} {}",
                truncate(&m.name, 10),
                m.hp,
                m.max_hp,
                m.type_label()
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

/// The detail view for one Pokémon: everything the game keeps to itself.
pub fn detail_lines<'a>(m: &Mon) -> Vec<Line<'a>> {
    let dim = Style::default().fg(Color::DarkGray);
    let frac = m.hp_fraction();
    let hp = m.hidden_power();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", truncate(&m.nickname, 10)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("({})", m.name), dim),
        ]),
        Line::from(vec![
            Span::raw(format!("#{:03}  L{}  ", m.dex, m.level)),
            Span::styled(m.type_label(), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("HP  "),
            Span::styled(hp_bar(frac, 12), Style::default().fg(hp_color(frac))),
            Span::raw(format!(" {}/{}", m.hp, m.max_hp)),
        ]),
        Line::from(Span::raw(format!(
            "ATK {:<4} DEF {:<4} SPD {:<4} SPC {}",
            m.attack, m.defense, m.speed, m.special
        ))),
        Line::from(Span::styled("── moves ──", dim)),
    ];
    for i in 0..4 {
        match m.moves.get(i) {
            Some(mv) => lines.push(Line::from(vec![
                Span::raw(format!("{:<14}", truncate(&mv.name, 14))),
                Span::styled(
                    format!("{:>2}/{:<2}", mv.pp, mv.pp_max),
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
                    dim,
                ),
            ])),
            // An empty move slot is genuinely empty, not unknown — so it gets a
            // dash rather than the unknown glyph.
            None => lines.push(Line::from(Span::styled("—", dim))),
        }
    }
    lines.push(Line::from(Span::styled("── hidden ──", dim)));
    lines.push(Line::from(Span::raw(format!(
        "DV {:>2}/{:>2}/{:>2}/{:>2}/{:>2}  {}/75",
        m.dvs.hp(),
        m.dvs.attack,
        m.dvs.defense,
        m.dvs.speed,
        m.dvs.special,
        m.dv_total()
    ))));
    lines.push(Line::from(Span::styled("   hp atk def spd spc", dim)));
    match &m.stat_exp {
        Some(e) => {
            lines.push(Line::from(Span::raw(format!(
                "EXP {:>5} {:>5} {:>5} {:>5} {:>5}",
                e.hp, e.attack, e.defense, e.speed, e.special
            ))));
        }
        // The in-battle structure does not carry stat experience, so during a
        // battle this is unknown rather than zero.
        None => lines.push(Line::from(vec![
            Span::raw("EXP "),
            Span::styled(format!("{UNKNOWN} (not in the battle copy)"), dim),
        ])),
    }
    lines.push(Line::from(vec![
        Span::raw("Hidden Power "),
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
        Span::styled("  not shiny if traded to Gen 2", dim)
    }));
    lines
}

/// The battle panel: the opponent, which is the thing being watched.
pub fn battle_lines<'a>(state: &TrainerState) -> Vec<Line<'a>> {
    let dim = Style::default().fg(Color::DarkGray);
    let Some(o) = state.opponent.as_ref() else {
        return Vec::new();
    };
    let kind = match state.battle {
        BattleKind::Wild => "WILD".to_string(),
        BattleKind::Trainer => {
            if state.trainer_name.is_empty() {
                "TRAINER".to_string()
            } else {
                format!("TRAINER {}", state.trainer_name)
            }
        }
        BattleKind::None => return Vec::new(),
    };
    let frac = o.hp_fraction();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("── {kind} ──"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                format!("{:<11}", truncate(&o.name, 11)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("L{:<3} ", o.level)),
            Span::styled(hp_bar(frac, 8), Style::default().fg(hp_color(frac))),
            Span::raw(format!(" {}/{}", o.hp, o.max_hp)),
        ]),
        Line::from(vec![
            Span::styled(o.type_label(), Style::default().fg(Color::Cyan)),
            Span::styled(
                match o.status.label() {
                    Some(s) => format!("  {s}"),
                    None => String::new(),
                },
                Style::default().fg(Color::Magenta),
            ),
        ]),
        Line::from(Span::raw(format!(
            "ATK {:<4} DEF {:<4} SPD {:<4} SPC {}",
            o.attack, o.defense, o.speed, o.special
        ))),
    ];
    let moves: Vec<String> = o
        .moves
        .iter()
        .map(|mv| format!("{} {}/{}", truncate(&mv.name, 12), mv.pp, mv.pp_max))
        .collect();
    if moves.is_empty() {
        // A freshly-encountered Pokémon whose moves have not loaded yet.
        lines.push(Line::from(Span::styled(format!("moves {UNKNOWN}"), dim)));
    } else {
        for chunk in moves.chunks(2) {
            lines.push(Line::from(Span::styled(chunk.join("  ·  "), dim)));
        }
    }
    lines
}

/// Which badges are still outstanding, named by their gym leader — the thing a
/// run watches after the party.
pub fn badge_lines<'a>(state: &TrainerState) -> Vec<Line<'a>> {
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
        Line::from(Span::styled(
            format!(
                "badges: {}",
                if got.is_empty() {
                    "none".to_string()
                } else {
                    got.join(", ")
                }
            ),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            match next {
                Some(leader) => format!("next: {leader}"),
                None => "all eight".to_string(),
            },
            Style::default().fg(Color::DarkGray),
        )),
    ]
}

/// The body block, so the panel reads as one thing.
pub fn body_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::gen1::{Dvs, Status};

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
        }
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
        // Header (5) + sprite (8) + four rows of text needs 17 rows; below that
        // the picture goes and the numbers stay.
        let short = layout(Rect::new(0, 0, PANEL_W, 16), true);
        assert_eq!(short.sprite.height, 0);
        assert!(short.body.height > 0, "the text must survive");
        let tall = layout(Rect::new(0, 0, PANEL_W, 17), true);
        assert_eq!(tall.sprite.height, SPRITE_ROWS);
    }

    #[test]
    fn asking_for_no_sprite_leaves_no_gap() {
        let l = layout(Rect::new(0, 0, PANEL_W, 40), false);
        assert_eq!(l.sprite.height, 0);
        assert_eq!(l.header.bottom(), l.body.y);
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
        let lines = party_lines(&st, 0);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn the_detail_view_marks_unknown_stat_experience_rather_than_zero() {
        // A battle-struct Pokémon has no stat experience; showing 0 would be a
        // lie, so it must show the unknown glyph.
        let lines = detail_lines(&mon(20, 41));
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains(UNKNOWN), "unknown stat exp is marked");
        assert!(text.contains("Hidden Power"));
        assert!(text.contains("DV"));
    }

    #[test]
    fn no_battle_produces_no_battle_panel() {
        let st = TrainerState::default();
        assert!(battle_lines(&st).is_empty());
    }

    #[test]
    fn money_is_grouped_for_reading() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(999999), "999,999");
    }
}

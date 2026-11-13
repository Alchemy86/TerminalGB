//! The one place a colour is given a *meaning*, and the layout primitives that
//! go with it.
//!
//! Every panel this frontend draws — the trainer view today, a battle panel
//! tomorrow — reads the same palette from here rather than spelling
//! `Color::Green` at a call site. That is not tidiness: a tracker is read at a
//! glance, and a glance only works if green means the same thing in every box
//! on the screen. Scattering literals is how `Color::Cyan` came to mean "a
//! section title" in one row and "a type" in the next.
//!
//! # Rules this module holds itself to
//!
//! * **Only the sixteen ANSI colours.** No `Rgb`, no `Indexed`. A 256-colour
//!   index does not *degrade* in an eight-colour terminal, it comes out
//!   arbitrary; the basic sixteen are the only ones every terminal, every
//!   theme and every `TERM` agrees about. See [`type_style`] for what that
//!   costs and how it is paid for.
//! * **Colour is never the only carrier.** Every value that is coloured is
//!   also spelled out, or marked with a glyph that differs as well
//!   (`●`/`○`, `+`/`-`, `★`). A colour-blind reader, a monochrome terminal and
//!   a screenshot in a black-and-white document all still work. The tests at
//!   the bottom of this file pin that for the cases where it would be easy to
//!   forget.
//! * **A style, not a colour, is the unit.** Two things that must be told
//!   apart may share a hue and differ by weight — which is what lets fifteen
//!   Gen 1 types live in fourteen usable colours without a collision a reader
//!   can actually hit.
//!
//! # Layout primitives
//!
//! [`section`] and [`column_header`] replace the centred `[ BANNER ]` rules the
//! trainer panel used to spend a row on each. A centred banner is decoration
//! that costs a line; a left-anchored label with a rule running off to the
//! right is a *column header*, which is the same line doing work.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Secondary text: labels, units, rules, anything the eye should skip over on
/// its way to a value.
pub fn label() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// A value the reader came for. Deliberately `Reset` rather than white, so it
/// is whatever the terminal's foreground is and stays legible on a light theme.
pub fn value() -> Style {
    Style::default().fg(Color::Reset)
}

/// A section title or a table's column header.
pub fn heading() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// The thing the cursor is on.
pub fn selected() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The other side of a fight: everything about the opponent.
pub fn foe() -> Style {
    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
}

/// Something good and worth noticing — a raised stat, a rising catch chance.
pub fn good() -> Style {
    Style::default().fg(Color::Green)
}

/// Something bad — a lowered stat, a move out of PP.
pub fn bad() -> Style {
    Style::default().fg(Color::Red)
}

/// The one style in this UI that shouts: black on bright yellow.
pub fn shout() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::LightYellow)
        .add_modifier(Modifier::BOLD)
}

/// A rarity mark — the `★` a Pokémon that would turn shiny in Gen 2 carries.
pub fn rare() -> Style {
    Style::default()
        .fg(Color::LightYellow)
        .add_modifier(Modifier::BOLD)
}

/// Remaining-HP colour, in three bands. More than three is noise at a glance,
/// and the thresholds are where a player's decision actually changes: half is
/// "think about a potion", a fifth is "this turn".
pub fn hp_color(fraction: f32) -> Color {
    if fraction <= 0.2 {
        Color::Red
    } else if fraction <= 0.5 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// A status condition, coloured by *how much it changes the next decision*
/// rather than by the game's own icon colours — which are a four-shade palette
/// and carry no hue at all.
///
/// Sleep and freeze stop the Pokémon acting, so they are the loud ones; poison
/// and burn tick away at it; paralysis is a coin-flip. `FNT` is spent.
pub fn status_style(code: &str) -> Style {
    match code {
        "SLP" | "FRZ" => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
        "PSN" => Style::default().fg(Color::Magenta),
        "BRN" => Style::default().fg(Color::LightRed),
        "PAR" => Style::default().fg(Color::Yellow),
        "FNT" => Style::default().fg(Color::DarkGray),
        _ => label(),
    }
}

/// A stat stage: raised is green, lowered is red. The sign is always printed
/// beside it (`ATK+2`, `DEF-1`), so the colour is reinforcement, never the
/// fact.
pub fn stage_style(stage: i8) -> Style {
    if stage < 0 {
        bad().add_modifier(Modifier::BOLD)
    } else {
        good().add_modifier(Modifier::BOLD)
    }
}

/// The fifteen Gen 1 types, each with its own style.
///
/// # Fourteen colours, fifteen types, and why that is not a problem
///
/// Restricting the palette to the basic ANSI sixteen (minus black and the
/// terminal's own foreground, which are already spoken for) leaves fourteen
/// usable hues. Two pairs therefore share one: `ELECTRIC`/`ROCK` on
/// `LightYellow` and `PSYCHIC`/`GHOST` on `LightMagenta`. They are separated by
/// **weight** — the first of each pair is bold — so the styles still differ,
/// and the type's *name* is printed in every place its colour appears, so the
/// colour is never carrying the fact on its own.
///
/// The pairs were chosen deliberately: no Gen 1 Pokémon is both `ELECTRIC` and
/// `ROCK`, and none is both `PSYCHIC` and `GHOST`, so the two halves of a dual
/// type can never collide with one another.
///
/// An unrecognised name (a hacked cartridge, a translation) gets [`value`] —
/// visible and uncoloured, which is the honest answer to "we do not know what
/// this is".
pub fn type_style(name: &str) -> Style {
    let plain = |c: Color| Style::default().fg(c);
    let bold = |c: Color| Style::default().fg(c).add_modifier(Modifier::BOLD);
    match name {
        "NORMAL" => plain(Color::Gray),
        "FIGHTING" => plain(Color::LightRed),
        "FLYING" => plain(Color::LightCyan),
        "POISON" => plain(Color::Magenta),
        "GROUND" => plain(Color::Yellow),
        "ROCK" => plain(Color::LightYellow),
        "BUG" => plain(Color::LightGreen),
        "GHOST" => plain(Color::LightMagenta),
        "FIRE" => plain(Color::Red),
        "WATER" => plain(Color::Blue),
        "GRASS" => plain(Color::Green),
        "ELECTRIC" => bold(Color::LightYellow),
        "PSYCHIC" => bold(Color::LightMagenta),
        "ICE" => plain(Color::Cyan),
        "DRAGON" => plain(Color::LightBlue),
        // `BIRD` is a real entry in Gen 1's type table that no obtainable
        // Pokémon has; a hack or a glitch Pokémon can show it.
        _ => value(),
    }
}

/// Every type name this palette knows, for the tests and for anything that
/// wants to render a legend.
pub const TYPE_NAMES: [&str; 15] = [
    "NORMAL",
    "FIGHTING",
    "FLYING",
    "POISON",
    "GROUND",
    "ROCK",
    "BUG",
    "GHOST",
    "FIRE",
    "WATER",
    "GRASS",
    "ELECTRIC",
    "PSYCHIC",
    "ICE",
    "DRAGON",
];

/// A left-anchored section header: `TEAM ─────────────────`.
///
/// This replaced a centred `[ TEAM ]` banner, and the difference is not
/// cosmetic. A centred banner is a row of decoration with a word in the middle
/// of it; the eye has to travel to find the word, and the row carries nothing
/// else. Anchoring the label left puts it exactly where the eye already is —
/// the left margin, where every other row starts — and the rule that follows
/// reads as "this heading covers everything to the right of here".
///
/// `trailer` is an optional right-aligned value (a count, a verdict) so the
/// header row can carry information rather than only naming the block.
pub fn section<'a>(
    title: &str,
    trailer: Option<Span<'a>>,
    width: usize,
) -> Line<'a> {
    let t: String = title.chars().collect();
    let tw = t.chars().count();
    if width <= tw {
        return Line::from(Span::styled(
            t.chars().take(width).collect::<String>(),
            heading(),
        ));
    }
    let tail = trailer.as_ref().map(|s| s.content.chars().count() + 1);
    let mut spans = vec![Span::styled(t, heading()), Span::styled(" ", label())];
    let used = tw + 1;
    let rule_w = width.saturating_sub(used + tail.unwrap_or(0));
    if rule_w > 0 {
        spans.push(Span::styled("─".repeat(rule_w), label()));
    }
    if let Some(s) = trailer {
        spans.push(Span::styled(" ", label()));
        spans.push(s);
    }
    Line::from(spans)
}

/// A plain full-width rule, for separating a footnote from the body.
pub fn divider<'a>(width: usize) -> Line<'a> {
    Line::from(Span::styled("─".repeat(width), label()))
}

/// A table's column header: a row label's worth of indent, then right-aligned
/// column names on the same grid the values below will use.
///
/// Tabulating is most of what makes IronMON readable at a glance, and a header
/// is what turns a row of numbers into a table. The caller supplies the same
/// `label_w` and `col_w` it will use for the value rows, so the two cannot
/// drift apart.
pub fn column_header<'a>(
    label_w: usize,
    cols: &[&str],
    col_w: usize,
    width: usize,
) -> Line<'a> {
    let mut text = " ".repeat(label_w);
    for c in cols {
        if text.chars().count() + col_w > width {
            break;
        }
        text.push_str(&format!("{c:>col_w$}"));
    }
    Line::from(Span::styled(text, label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette is only useful if it degrades. An indexed or RGB colour is
    /// approximated arbitrarily by an eight- or sixteen-colour terminal, so
    /// nothing here may use one.
    #[test]
    fn every_colour_is_one_of_the_basic_ansi_sixteen() {
        let basic = |c: Option<Color>| match c {
            None => true,
            Some(Color::Rgb(..)) | Some(Color::Indexed(_)) => false,
            Some(_) => true,
        };
        let mut styles = vec![
            label(),
            value(),
            heading(),
            selected(),
            foe(),
            good(),
            bad(),
            shout(),
            rare(),
            stage_style(-1),
            stage_style(2),
        ];
        styles.extend(TYPE_NAMES.iter().map(|t| type_style(t)));
        styles.extend(
            ["SLP", "PSN", "BRN", "FRZ", "PAR", "FNT", ""]
                .iter()
                .map(|s| status_style(s)),
        );
        for s in styles {
            assert!(basic(s.fg) && basic(s.bg), "{s:?} is not a basic colour");
        }
        for f in [0.0f32, 0.1, 0.3, 0.6, 1.0] {
            assert!(basic(Some(hp_color(f))));
        }
    }

    /// Two types that a reader has to tell apart must not render identically.
    /// Where the colour is shared the weight is not, which is what makes
    /// fifteen types fit in fourteen hues.
    #[test]
    fn no_two_types_render_the_same_way() {
        for (i, a) in TYPE_NAMES.iter().enumerate() {
            for b in &TYPE_NAMES[i + 1..] {
                assert_ne!(
                    (type_style(a).fg, type_style(a).add_modifier),
                    (type_style(b).fg, type_style(b).add_modifier),
                    "{a} and {b} are indistinguishable"
                );
            }
        }
    }

    /// The two pairs that share a hue are the two that can never occur together
    /// on one Pokémon. If this ever changes, the palette has to.
    #[test]
    fn the_types_that_share_a_colour_never_share_a_pokemon() {
        let shared: Vec<(&str, &str)> = TYPE_NAMES
            .iter()
            .enumerate()
            .flat_map(|(i, a)| {
                TYPE_NAMES[i + 1..]
                    .iter()
                    .filter(move |b| type_style(a).fg == type_style(b).fg)
                    .map(move |b| (*a, *b))
            })
            .collect();
        assert_eq!(
            shared,
            vec![("ROCK", "ELECTRIC"), ("GHOST", "PSYCHIC")],
            "a new colour collision needs the same argument making for it"
        );
    }

    /// A status must never be readable by colour alone: the label is three
    /// letters and is always printed. This pins that every status the game can
    /// report has a *distinct* label, which is the part colour is reinforcing.
    #[test]
    fn a_status_is_named_as_well_as_coloured() {
        let labels = ["SLP", "PSN", "BRN", "FRZ", "PAR", "FNT"];
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert_ne!(a, b);
            }
            // …and every *live* status is styled deliberately rather than
            // falling through. `FNT` is the exception on purpose: a fainted
            // Pokémon is spent, and dim is what "spent" looks like everywhere
            // else in this panel.
            if *a != "FNT" {
                assert_ne!(status_style(a), label(), "{a} has no style of its own");
            }
        }
    }

    #[test]
    fn a_section_header_is_exactly_the_width_asked_for() {
        for w in 1usize..=80 {
            let l = section("PARTY", None, w);
            let n: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(n, w.min(w), "width {w} rendered {n}");
        }
        let text: String = section("PARTY", None, 20)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, "PARTY ──────────────");
        // …and a trailer sits at the far right rather than pushing the rule off.
        let text: String = section("PARTY", Some(Span::raw("4/6")), 20)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, "PARTY ────────── 4/6");
        assert_eq!(text.chars().count(), 20);
    }

    #[test]
    fn a_column_header_lines_up_with_its_values() {
        let head: String = column_header(5, &["HP", "ATK", "DEF"], 5, 40)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(head, "        HP  ATK  DEF");
        // The value row the caller writes with the same widths lands under it.
        let row = format!("{:<5}{:>5}{:>5}{:>5}", "stat", 34, 42, 38);
        assert_eq!(row.len(), head.len());
        assert_eq!(head.find("ATK").unwrap() + 3, row.find("42").unwrap() + 2);
    }
}

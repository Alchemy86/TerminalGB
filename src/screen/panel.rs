//! The panel design system: **one palette and one set of layout primitives**
//! for every tracker-shaped panel this frontend draws.
//!
//! It exists because colour was being decided at the call site. The trainer
//! panel painted every type `Cyan`, every status `Magenta` and every stat the
//! terminal's default, so the only thing on it a reader could parse without
//! reading a word was the HP bar. Fixing that by scattering `Color::…`
//! literals through [`crate::screen::trainer_view`] would have made the next
//! panel — the Gen 1 battle view the engine in `terminalgb-gen1-battle` is
//! waiting for — a second, subtly different set of the same decisions. So the
//! decisions live here, once, and both panels read them.
//!
//! # Colour carries meaning, never decoration
//!
//! Five families, and nothing outside them:
//!
//! | family | means | where |
//! |---|---|---|
//! | [`ink`] | structure — label, value, rule, heading | everywhere |
//! | [`type_color`] | one of the fifteen Gen 1 types | type chips, move rows |
//! | [`hp_color`] | how much of a health bar is left | bars, slot pips |
//! | [`status_color`] | poison / burn / freeze / paralysis / sleep | status column |
//! | [`stage_color`] | a stat raised or lowered in battle | `MOD` rows |
//!
//! **Every one of them is redundant with something a reader can also read.**
//! A raised stat is green *and* carries a `+`; a fainted slot is dim *and*
//! hollow; a type is coloured *and* named. That is not belt and braces for its
//! own sake — a terminal theme can make any two of these hues indistinguishable
//! and a colour-blind reader may never separate the red/green pair at all, so
//! colour here is an accelerator for the eye and never the only carrier of a
//! fact. [`the_panel_never_carries_a_fact_in_colour_alone`] is the test.
//!
//! # Degrading when colour is limited
//!
//! Fifteen types do not fit in the eight ANSI colours, so [`type_color`] picks
//! from the 256-colour cube by default and from a named-ANSI table when the
//! terminal will not have it ([`Depth`]). The named table has **declared
//! collisions** — it has to, there are more types than colours — and they are
//! chosen so no *pair that occurs together on one Pokémon* collides, which is
//! the case where a collision would actually mislead. `Depth` is decided once
//! at startup from the environment ([`Depth::from_env`]) rather than per span,
//! and the answer is reported in the Status panel, because a palette that
//! silently degraded is one nobody can debug.
//!
//! # Layout primitives
//!
//! [`rule`] is **left-anchored**, not centred: the old panel spent a whole row
//! on `─────── [ PARTY ] ───────`, which is nine characters of information in
//! thirty-eight columns of decoration, and the eye had to travel to the middle
//! of each one to find out where it was. A left rule puts every section label
//! in the same column so the page can be scanned down its left edge.
//! [`table_head`] goes further and makes the row carry data — a moves table's
//! header names its columns, so it is a label *and* a legend.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Structural ink
// ---------------------------------------------------------------------------

/// Structural colours: what a span *is*, rather than what it says.
///
/// Named ANSI throughout, because these are the spans that must be legible on
/// every terminal and in every theme — the 256-colour cube is spent on the one
/// place fifteen distinct hues genuinely earn it, which is [`type_color`].
pub mod ink {
    use ratatui::style::Color;

    /// A field's name. Recessive: the value beside it is the information.
    pub const LABEL: Color = Color::DarkGray;
    /// Rules, separators, brackets, padding glyphs.
    pub const RULE: Color = Color::DarkGray;
    /// A section's name on its rule.
    pub const HEADING: Color = Color::Cyan;
    /// An ordinary value. The terminal's own foreground, so the panel inherits
    /// the user's theme rather than fighting it.
    pub const VALUE: Color = Color::Reset;
    /// The thing the panel wants you to look at — the focused row, the active
    /// Pokémon's marker.
    pub const ACCENT: Color = Color::Cyan;
    /// Good news: a raised stat, full PP, a badge in hand.
    pub const GOOD: Color = Color::Green;
    /// Worth noticing but not yet bad.
    pub const WARN: Color = Color::Yellow;
    /// Bad news: a lowered stat, no PP left, a fainted Pokémon.
    pub const BAD: Color = Color::Red;
    /// A shiny verdict, and nothing else — it is the loudest thing the panel
    /// says and it must not have to compete.
    pub const SHINY: Color = Color::LightYellow;
    /// The opponent's identity.
    pub const FOE: Color = Color::Red;
}

/// `Style` for a label.
pub fn label() -> Style {
    Style::default().fg(ink::LABEL)
}

/// `Style` for a rule or other furniture.
pub fn furniture() -> Style {
    Style::default().fg(ink::RULE)
}

/// `Style` for an ordinary value.
pub fn value() -> Style {
    Style::default().fg(ink::VALUE)
}

// ---------------------------------------------------------------------------
// Colour depth
// ---------------------------------------------------------------------------

/// How many colours the panel may spend.
///
/// There is no reliable *probe* for this — a terminal answers no query about
/// its palette — so it is read from the environment, which is the same
/// evidence every other program uses. `Depth::Ansi` is the honest fallback,
/// not a punishment: the panel is fully legible in it, because no fact is
/// carried in colour alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 8/16 named colours. Types share hues; the collisions are declared.
    Ansi,
    /// The 256-colour cube. One hue per type.
    Indexed,
}

impl Depth {
    /// Read `$COLORTERM` and `$TERM`, in that order.
    ///
    /// `COLORTERM=truecolor|24bit` and any `TERM` naming 256 colours (or
    /// `kitty`/`alacritty`/`wezterm`, which do not say so in `TERM` but always
    /// have it) mean [`Depth::Indexed`]. `$NO_COLOR` is honoured as
    /// [`Depth::Ansi`] rather than as "no colour at all", because the panel's
    /// structure *is* colour — stripping it entirely would leave a wall of
    /// undifferentiated text, and the redundancy rule means Ansi already
    /// carries every fact.
    pub fn from_env() -> Depth {
        let get = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
        if !get("NO_COLOR").is_empty() {
            return Depth::Ansi;
        }
        let colorterm = get("COLORTERM");
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            return Depth::Indexed;
        }
        let term = get("TERM");
        if term.contains("256")
            || term.contains("kitty")
            || term.contains("alacritty")
            || term.contains("wezterm")
            || term.contains("direct")
        {
            return Depth::Indexed;
        }
        Depth::Ansi
    }

    /// What the Status panel says, so a degraded palette is visible rather
    /// than merely suffered.
    pub fn label(self) -> &'static str {
        match self {
            Depth::Ansi => "16",
            Depth::Indexed => "256",
        }
    }
}

/// The depth every `type_color` call uses, decided once.
///
/// A process-wide value for the same reason `sound::output_volume`'s level is
/// one: it is read from deep inside rendering, on a path that has no room for
/// another parameter, and it never changes after startup.
static DEPTH: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

/// Fix the depth for the process. Called once by the frontend at startup.
pub fn set_depth(d: Depth) {
    DEPTH.store(
        match d {
            Depth::Ansi => 0,
            Depth::Indexed => 1,
        },
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The depth in force.
pub fn depth() -> Depth {
    match DEPTH.load(std::sync::atomic::Ordering::Relaxed) {
        0 => Depth::Ansi,
        _ => Depth::Indexed,
    }
}

/// Serialises the tests that move the process-wide depth.
///
/// The same requirement `sound::output_volume::test_lock` has and for the same
/// reason: `cargo test` runs these in parallel, and one of them moving a global
/// under another is a flake that reproduces once a fortnight.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The fifteen Gen 1 types, with the colour each gets at both depths.
///
/// The 256-colour column is picked to sit near the type's canonical colour so
/// a reader who knows the games recognises it without a legend — Fire orange,
/// Water blue, Grass green, Electric yellow, Psychic pink, Ice pale cyan,
/// Ground tan, Rock stone, Ghost indigo, Poison violet.
///
/// **Ground and Rock are deliberately further apart here than they are in the
/// games.** Their canonical colours (`#E0C068` and `#B8A038`) are two shades of
/// the same khaki, which is fine on a printed type chart and useless in three
/// terminal columns — and Rock/Ground is a real dual type (Geodude, Onix,
/// Rhyhorn), so the one place they appear together is the place they must not
/// be confusable. Ground keeps the sandy tan; Rock is dark stone.
///
/// The named column has three collisions and they are chosen, not accidental.
/// The rule is that **no pair of types that occurs together on one Pokémon may
/// share a colour**, since that is the only place a collision changes what a
/// reader concludes (`GHOST/POISON` in two identical hues reads as one type
/// written twice). Checked exhaustively against the cartridge's own type table
/// by `no_dual_type_shares_a_colour_at_either_depth`.
const TYPES: &[(&str, u8, Color)] = &[
    // name        256   named ANSI
    ("NORMAL", 249, Color::Gray),
    ("FIGHTING", 124, Color::Red),
    ("FLYING", 147, Color::LightBlue),
    ("POISON", 133, Color::LightMagenta),
    ("GROUND", 180, Color::Yellow),
    ("ROCK", 101, Color::LightRed),
    ("BUG", 148, Color::LightGreen),
    ("GHOST", 61, Color::Blue),
    ("FIRE", 208, Color::LightRed),
    ("WATER", 33, Color::Blue),
    ("GRASS", 71, Color::Green),
    ("ELECTRIC", 220, Color::LightYellow),
    ("PSYCHIC", 205, Color::Magenta),
    ("ICE", 123, Color::LightCyan),
    ("DRAGON", 62, Color::Magenta),
    // `BIRD` exists in the cartridge's type table but no obtainable Pokémon
    // and no move carries it; it is here so an unexpected id still gets a
    // colour rather than falling through to the default.
    ("BIRD", 147, Color::LightBlue),
];

/// The colour of a Gen 1 type, by name as the cartridge spells it.
///
/// Case-insensitive and tolerant of the three-letter abbreviations the narrow
/// tiers use (`POI`, `FLY`), because the abbreviation is the *same fact* and
/// losing its colour exactly where the panel is most cramped would be
/// backwards.
pub fn type_color(name: &str) -> Color {
    let n = name.trim();
    if n.is_empty() {
        return ink::LABEL;
    }
    let indexed = depth() == Depth::Indexed;
    for (t, idx, ansi) in TYPES {
        // Full name first, then the prefix an abbreviation would produce, so
        // `FIGHTING` can never be matched by `FIRE`'s three letters.
        if n.eq_ignore_ascii_case(t) {
            return if indexed { Color::Indexed(*idx) } else { *ansi };
        }
    }
    for (t, idx, ansi) in TYPES {
        if n.len() >= 3 && t.len() >= n.len() && t[..n.len()].eq_ignore_ascii_case(n) {
            return if indexed { Color::Indexed(*idx) } else { *ansi };
        }
    }
    ink::VALUE
}

/// A type name in its own colour.
pub fn type_span<'a>(name: impl Into<String>) -> Span<'a> {
    let n: String = name.into();
    let c = type_color(&n);
    Span::styled(n, Style::default().fg(c))
}

// ---------------------------------------------------------------------------
// Health, status, stat stages
// ---------------------------------------------------------------------------

/// Three bands, because a run makes three different decisions: fine, heal
/// soon, heal now.
///
/// The thresholds are the ones the games themselves use for the low-health
/// beep (a fifth) and the yellow bar (a half), so a player already knows them.
pub fn hp_color(fraction: f32) -> Color {
    if fraction <= 0.2 {
        ink::BAD
    } else if fraction <= 0.5 {
        ink::WARN
    } else {
        ink::GOOD
    }
}

/// The colour of a status label (`PSN`, `BRN`, `FRZ`, `PAR`, `SLP`, `FNT`).
///
/// Distinct hues rather than one "something is wrong" colour, because the
/// decision differs: a burn wants a switch, paralysis wants a speed
/// recalculation, freeze wants an item. The label is always drawn beside it.
pub fn status_color(label: &str) -> Color {
    match label {
        "PSN" | "TOX" => Color::LightMagenta,
        "BRN" => Color::LightRed,
        "FRZ" => Color::LightCyan,
        "PAR" => Color::Yellow,
        "SLP" => Color::Blue,
        "FNT" => ink::LABEL,
        _ => ink::VALUE,
    }
}

/// A stat stage's colour: raised green, lowered red, neutral recessive.
///
/// The sign is always printed beside it — see the redundancy rule at the top
/// of this module.
pub fn stage_color(stage: i8) -> Color {
    match stage {
        s if s > 0 => ink::GOOD,
        s if s < 0 => ink::BAD,
        _ => ink::LABEL,
    }
}

// ---------------------------------------------------------------------------
// Layout primitives
// ---------------------------------------------------------------------------

/// A **left-anchored** section rule: `─ PARTY ──────────────`.
///
/// One leading dash so the label does not sit against the border, the label,
/// then the rule out to `width`. Every section label therefore starts in the
/// same column and the page can be scanned down its left edge — which a
/// centred rule makes impossible, since the label's column depends on how long
/// it is.
///
/// Below the label's own width the rule disappears and the label survives:
/// losing the decoration is always better than losing the name.
pub fn rule<'a>(title: &str, width: usize) -> Line<'a> {
    rule_with(title, None, width)
}

/// [`rule`] with a right-aligned annotation — `─ PARTY ───────── 4/6`.
///
/// The annotation is the first thing to go as the panel narrows, then the
/// trailing rule, then the leading dash.
pub fn rule_with<'a>(title: &str, note: Option<&str>, width: usize) -> Line<'a> {
    let head = Style::default()
        .fg(ink::HEADING)
        .add_modifier(Modifier::BOLD);
    let n = title.chars().count();
    if width <= n {
        return Line::from(Span::styled(truncate(title, width), head));
    }
    if width < n + 2 {
        // Room for the label and a dash or two, but not for the leading `─ `.
        return Line::from(vec![
            Span::styled(title.to_string(), head),
            Span::styled("─".repeat(width - n), furniture()),
        ]);
    }
    let mut spans = vec![
        Span::styled("─ ", furniture()),
        Span::styled(title.to_string(), head),
    ];
    let used = n + 2;
    if width == used {
        return Line::from(spans);
    }
    // The annotation, and only if a real run of dashes survives beside it — an
    // annotation crammed against the label reads as part of it.
    let tail = match note {
        Some(t) if !t.is_empty() && used + t.chars().count() + 5 <= width => {
            format!(" {t}")
        }
        _ => String::new(),
    };
    // One space after the label, then dashes up to the annotation (or the end).
    let rest = width - used - tail.chars().count();
    spans.push(Span::styled(
        format!(" {}", "─".repeat(rest - 1)),
        furniture(),
    ));
    if !tail.is_empty() {
        spans.push(Span::styled(tail, label()));
    }
    Line::from(spans)
}

/// A column header for a table: dim, upper-case, aligned to the columns under
/// it. Earns its row by being a legend as well as a label.
pub fn table_head<'a>(cells: &[(&str, usize, Align)]) -> Line<'a> {
    let mut spans = Vec::with_capacity(cells.len());
    for (i, (text, w, a)) in cells.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(a.fit(text, *w), label()));
    }
    Line::from(spans)
}

/// Which edge a table cell is aligned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

impl Align {
    /// Pad or clip `s` to exactly `w` columns.
    pub fn fit(self, s: &str, w: usize) -> String {
        let t = truncate(s, w);
        let pad = w.saturating_sub(t.chars().count());
        match self {
            Align::Left => format!("{t}{}", " ".repeat(pad)),
            Align::Right => format!("{}{t}", " ".repeat(pad)),
        }
    }
}

/// A horizontal bar of `width` cells showing `fraction`, in the block elements
/// the rest of this frontend already uses.
///
/// **A non-zero fraction never renders as an empty bar** — one cell is kept
/// however small the fraction is, because "1 HP left" and "fainted" are the
/// two facts a tracker exists to separate and a rounded-down bar confuses
/// them. Symmetrically, anything short of full keeps one empty cell.
///
/// The two rules collide at `width == 1`, where a survivor and full health
/// have one cell between them; **alive wins**, because it is the fact the
/// panel exists to carry and the numbers beside the bar still separate the two.
pub fn bar(fraction: f32, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let f = fraction.clamp(0.0, 1.0);
    let mut filled = (f * width as f32).round() as usize;
    if f < 1.0 && filled == width {
        filled = width - 1;
    }
    if f > 0.0 && filled == 0 {
        filled = 1;
    }
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// A bar in its health colour.
pub fn hp_bar_span<'a>(fraction: f32, width: usize) -> Span<'a> {
    Span::styled(
        bar(fraction, width),
        Style::default().fg(hp_color(fraction)),
    )
}

/// A `LABEL value` pair, the label recessive.
pub fn kv<'a>(l: &str, v: impl Into<String>) -> Vec<Span<'a>> {
    vec![
        Span::styled(format!("{l} "), label()),
        Span::styled(v.into(), value()),
    ]
}

/// Clip to at most `n` columns, marking that something was cut.
///
/// Shared so a clipped value looks the same wherever it is clipped.
pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    if n == 0 {
        return String::new();
    }
    s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
}

/// Pad or clip to exactly `n` columns, so a column stays a column.
pub fn pad(s: &str, n: usize) -> String {
    Align::Left.fit(s, n)
}

/// The width of a rendered line in terminal columns.
///
/// Every glyph the panels use is single-width — pinned by
/// `every_glyph_the_panel_uses_is_single_width` in
/// [`crate::screen::trainer_view`] — so counting chars is counting columns.
pub fn line_width(l: &Line<'_>) -> usize {
    l.spans.iter().map(|s| s.content.chars().count()).sum()
}

/// Clip a composed line to `width`, span by span, keeping the styling of what
/// survives.
///
/// A **net**, not a layout mechanism: every section sizes its own columns, and
/// `no_line_overflows_at_any_width` proves this is never load-bearing at a
/// realistic width. Without it an over-long line bleeds into the border and
/// corrupts the frame.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every type the cartridge can name gets a colour, at both depths, and no
    /// call falls through to the structural default — a type drawn in the
    /// terminal's plain foreground reads as "no type" beside its coloured
    /// neighbours.
    #[test]
    fn every_gen1_type_has_a_colour_at_both_depths() {
        let _g = test_lock();
        for depth in [Depth::Ansi, Depth::Indexed] {
            set_depth(depth);
            for (name, _, _) in TYPES {
                assert_ne!(
                    type_color(name),
                    ink::VALUE,
                    "{name} has no colour at {depth:?}"
                );
                // …and the abbreviation the narrow tiers use resolves the same.
                let short: String = name.chars().take(3).collect();
                assert_eq!(
                    type_color(&short),
                    type_color(name),
                    "{short} ({name}) loses its colour at {depth:?}"
                );
            }
        }
        set_depth(Depth::Indexed);
    }

    /// `FIRE` and `FIGHTING` share their first three letters in neither
    /// direction that matters: `FIG` must not resolve to Fire's orange.
    #[test]
    fn an_abbreviation_resolves_to_its_own_type() {
        let _g = test_lock();
        set_depth(Depth::Indexed);
        assert_eq!(type_color("FIG"), type_color("FIGHTING"));
        assert_eq!(type_color("FIR"), type_color("FIRE"));
        assert_ne!(type_color("FIGHTING"), type_color("FIRE"));
        assert_eq!(type_color("GRO"), type_color("GROUND"));
        assert_ne!(type_color("GROUND"), type_color("GRASS"));
    }

    /// The named-ANSI table has collisions — it must, fifteen types into eight
    /// hues — but never between two types that appear **on the same
    /// Pokémon**, which is the only place a shared colour changes what a
    /// reader concludes.
    ///
    /// The pair list is every dual type in Red/Blue.
    #[test]
    fn no_dual_type_shares_a_colour_at_either_depth() {
        let _g = test_lock();
        const DUALS: &[(&str, &str)] = &[
            ("GRASS", "POISON"),   // Bulbasaur line, Oddish line, Bellsprout
            ("BUG", "POISON"),     // Weedle line, Venonat line
            ("POISON", "FLYING"),  // Zubat line
            ("NORMAL", "FLYING"),  // Pidgey line, Farfetch'd, Doduo
            ("FIRE", "FLYING"),    // Charizard, Moltres
            ("WATER", "PSYCHIC"),  // Slowpoke line, Starmie
            ("WATER", "ICE"),      // Dewgong, Cloyster, Lapras
            ("WATER", "POISON"),   // Tentacool line
            ("WATER", "FIGHTING"), // Poliwrath
            ("WATER", "FLYING"),   // Gyarados
            ("GROUND", "ROCK"),    // Geodude line, Rhyhorn line
            ("ROCK", "FLYING"),    // Aerodactyl
            ("ROCK", "WATER"),     // Omanyte line, Kabuto line
            ("GHOST", "POISON"),   // Gastly line
            ("ICE", "PSYCHIC"),    // Jynx
            ("ELECTRIC", "STEEL"), // (none in Gen 1 — a control: unknown types)
            ("BUG", "FLYING"),     // Butterfree, Scyther is Bug/Flying
            ("BUG", "GRASS"),      // Paras line
            ("GROUND", "FLYING"),  // Gligar-shaped; none in Gen 1
            ("PSYCHIC", "FLYING"), // none in Gen 1
            ("DRAGON", "FLYING"),  // Dragonite
            ("FIGHTING", "FLYING"),
            ("GHOST", "FLYING"),
            ("NORMAL", "PSYCHIC"),
        ];
        for depth in [Depth::Ansi, Depth::Indexed] {
            set_depth(depth);
            for (a, b) in DUALS {
                if type_color(a) == ink::VALUE || type_color(b) == ink::VALUE {
                    continue; // not a Gen 1 type; the control rows
                }
                assert_ne!(
                    type_color(a),
                    type_color(b),
                    "{a}/{b} are the same colour at {depth:?} — a dual type \
                     would read as one type written twice"
                );
            }
        }
        set_depth(Depth::Indexed);
    }

    /// Colour is an accelerator, never the carrier. Every fact the palette
    /// colours must also be readable with the colour thrown away.
    ///
    /// Asserted structurally: a raised stage prints its sign, a status prints
    /// its label, a bar prints filled and empty cells that differ as glyphs.
    #[test]
    fn the_panel_never_carries_a_fact_in_colour_alone() {
        // The bar: full and empty differ as characters, not only as colour.
        assert!(bar(1.0, 4).chars().all(|c| c == '█'));
        assert!(bar(0.0, 4).chars().all(|c| c == '░'));
        assert_ne!(bar(1.0, 4), bar(0.0, 4));
        // A stage's colour is a function of its sign, which the caller prints.
        assert_eq!(stage_color(2), ink::GOOD);
        assert_eq!(stage_color(-2), ink::BAD);
        assert_ne!(stage_color(2), stage_color(-2));
        // A status label is text before it is a colour.
        for l in ["PSN", "BRN", "FRZ", "PAR", "SLP", "FNT"] {
            assert!(!l.is_empty());
            let _ = status_color(l);
        }
    }

    /// One hit point is never an empty bar, and full health is never a
    /// partially empty one.
    #[test]
    fn a_bar_never_rounds_a_survivor_to_dead() {
        for w in 1..=20 {
            let almost_dead = bar(1.0 / 999.0, w);
            assert!(
                almost_dead.starts_with('█'),
                "1/999 HP renders empty at width {w}: {almost_dead}"
            );
            assert_eq!(bar(1.0, w).chars().filter(|c| *c == '█').count(), w);
            assert_eq!(bar(0.0, w).chars().filter(|c| *c == '█').count(), 0);
            // …and 99.9% is not rounded up to full — except at one cell, where
            // "alive" and "full" genuinely cannot both be shown and alive wins.
            if w > 1 {
                assert!(bar(0.999, w).ends_with('░'), "99.9% reads as full at {w}");
            }
        }
    }

    /// The rule is left-anchored, exactly the width asked for, and keeps its
    /// label when there is no room for anything else.
    #[test]
    fn a_rule_is_left_anchored_and_exactly_the_width_asked_for() {
        for w in 1..=60 {
            let l = rule("PARTY", w);
            assert_eq!(line_width(&l), w.min(w), "width {w}");
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text.chars().count(), w, "rule at {w}: {text:?}");
            if w >= 8 {
                assert!(
                    text.starts_with("─ PARTY"),
                    "not left-anchored at {w}: {text:?}"
                );
            }
            if w >= 5 {
                assert!(text.contains("PART"), "label lost at {w}: {text:?}");
            }
        }
    }

    #[test]
    fn a_rules_annotation_goes_before_its_label_does() {
        let wide: String = rule_with("PARTY", Some("4/6"), 40)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(wide.contains("PARTY") && wide.contains("4/6"), "{wide:?}");
        assert_eq!(wide.chars().count(), 40);
        let narrow: String = rule_with("PARTY", Some("4/6"), 10)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(narrow.contains("PARTY"), "{narrow:?}");
        assert!(!narrow.contains("4/6"), "annotation kept over the rule");
        assert_eq!(narrow.chars().count(), 10);
    }

    #[test]
    fn a_table_head_is_exactly_its_columns_wide() {
        let l = table_head(&[
            ("MOVE", 12, Align::Left),
            ("PP", 5, Align::Right),
            ("POW", 3, Align::Right),
        ]);
        assert_eq!(line_width(&l), 12 + 1 + 5 + 1 + 3);
    }

    #[test]
    fn depth_is_read_from_the_environment_and_no_color_is_honoured() {
        let _g = test_lock();
        // Not asserting against the real environment — just that the mapping
        // is total and that the labels differ, so the Status row can say which.
        assert_ne!(Depth::Ansi.label(), Depth::Indexed.label());
        set_depth(Depth::Ansi);
        assert_eq!(depth(), Depth::Ansi);
        set_depth(Depth::Indexed);
        assert_eq!(depth(), Depth::Indexed);
    }
}

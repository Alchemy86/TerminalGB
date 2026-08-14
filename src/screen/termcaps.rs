//! What graphics protocol does this terminal actually speak?
//!
//! **Ask; never infer from `$TERM`.** The same rule the shared-memory handshake
//! already follows ([`crate::screen::tui`]) applies to protocol selection: a
//! rule of the form "if the terminal is called X it supports Y" is wrong in a
//! way the user sees as a blank rectangle or a coarse picture, and it is wrong
//! in both directions (WezTerm speaks the Kitty graphics protocol but *not* its
//! Unicode placeholders; Ptyxis and GNOME Console are "modern" terminals with no
//! graphics protocol at all).
//!
//! This module is the **pure** half: it builds the query string and parses the
//! reply. It touches no file descriptor, so every rule below is unit-tested
//! against captured bytes from real terminals instead of against a live
//! terminal that has to be present for the test to mean anything.
//!
//! # Why we ask ourselves instead of letting `ratatui-image` ask
//!
//! `Picker::from_query_stdio()` asks the same three questions, but it returns
//! `Err(NoFontSize)` — **discarding the capability answer it just received** —
//! when the terminal does not report a cell size. Two things have to fail for
//! that, and on VTE both do:
//!
//! * `CSI 16 t` (cell size in pixels) is unanswered. Measured on VTE 0.84.1
//!   (Ptyxis, Fedora 44): silence.
//! * `TIOCGWINSZ` reports `ws_xpixel = ws_ypixel = 0`, so the library's fallback
//!   yields nothing either. Measured on the same terminal: `cols=112 rows=35
//!   xpixel=0 ypixel=0`.
//!
//! The caller then has nothing left but `Picker::from_fontsize((10, 20))`, whose
//! protocol is `Halfblocks` — so a terminal that *did* advertise Sixel or Kitty
//! but happens not to answer `CSI 16 t` silently loses its protocol for the whole
//! session. That is the defect this module exists to remove.
//!
//! Two answers make the cell size recoverable where the library gives up:
//!
//! * `CSI 14 t` → window size in pixels, and
//! * `CSI 18 t` → window size in cells.
//!
//! VTE answers both (`ESC[4;700;1008t` and `ESC[8;35;112t`), so its cell is
//! 1008/112 × 700/35 = **9 × 20** px — a real measurement instead of the
//! fabricated 10 × 20.
//!
//! # References
//!
//! * Kitty graphics protocol, "Querying support and available transmission
//!   mediums" — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
//! * DEC Device Attributes; the Sixel extension is parameter `4`. See
//!   `XTerm Control Sequences`, "Send Device Attributes (Primary DA)" —
//!   <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>
//! * XTWINOPS `CSI 14 t` / `CSI 16 t` / `CSI 18 t`, same document, "Window
//!   manipulation".

/// A graphics protocol, best first.
///
/// The order of the variants **is** the preference order, and
/// [`TermCaps::best`] relies on it: Kitty places a cell-addressed image and can
/// hand a frame over in shared memory (~15 bytes/frame); Sixel draws real pixels
/// but re-transmits the whole picture every frame; iTerm2 is cell-addressed but
/// cannot be probed (it is an env-var guess); half-blocks is text and is the
/// true last resort — two pixels per character cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Graphics {
    Kitty,
    Sixel,
    Iterm2,
    Halfblocks,
}

/// Everything one round trip to the terminal establishes about its graphics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermCaps {
    /// The terminal answered the Kitty graphics `a=q` handshake with `OK`.
    pub kitty: bool,
    /// Primary DA listed `4` — the Sixel extension.
    pub sixel: bool,
    /// Cell size in pixels, from `CSI 16 t` or derived from `CSI 14 t` +
    /// `CSI 18 t`. `None` means the terminal reported neither.
    pub cell: Option<(u16, u16)>,
    /// The terminal answered *something*. A probe that timed out completely
    /// tells us nothing, and must not be read as "supports nothing" — see
    /// [`TermCaps::answered`].
    pub answered: bool,
}

impl TermCaps {
    /// The best protocol the terminal actually advertised, or `None` when it
    /// advertised neither of the two that can be probed.
    ///
    /// Returning `None` rather than `Halfblocks` is deliberate: it leaves the
    /// caller free to fall back to the iTerm2 env guess, which sits *above*
    /// half-blocks in the ladder but cannot be established by asking.
    pub fn best(&self) -> Option<Graphics> {
        if self.kitty {
            Some(Graphics::Kitty)
        } else if self.sixel {
            Some(Graphics::Sixel)
        } else {
            None
        }
    }
}

/// The image id used for the Kitty capability question.
///
/// Distinct from the shared-memory probe's id so the two replies cannot be
/// confused for one another when both are in flight in the same round trip.
pub const KITTY_QUERY_ID: u32 = 0x0067_620a;

/// Build the capability query.
///
/// Ordered cheapest-to-answer last so the `CSI 6 n` fence really is the final
/// byte the terminal produces: everything before it has already been answered
/// (or ignored) by the time the cursor report arrives, which is what lets
/// *silence* be a conclusive "no" instead of "we did not wait long enough".
///
/// `tmux` requires the escapes to be escaped and the whole run wrapped in its
/// passthrough sequence, exactly as the shared-memory query already does.
pub fn query(is_tmux: bool) -> String {
    let (start, esc, end) = if is_tmux {
        ("\x1bPtmux;", "\x1b\x1b", "\x1b\\")
    } else {
        ("", "\x1b", "")
    };
    let mut q = String::with_capacity(96);
    q.push_str(start);
    // Kitty graphics: a 1×1 direct transfer, queried not drawn.
    q.push_str(esc);
    q.push_str(&format!("_Gi={KITTY_QUERY_ID},s=1,v=1,a=q,t=d,f=24;AAAA"));
    q.push_str(esc);
    q.push('\\');
    // Primary DA — Sixel is parameter 4.
    q.push_str(esc);
    q.push_str("[c");
    // Cell size in pixels; then the two that let us derive it if that is silent.
    q.push_str(esc);
    q.push_str("[16t");
    q.push_str(esc);
    q.push_str("[14t");
    q.push_str(esc);
    q.push_str("[18t");
    // The fence. Every terminal answers it, and it arrives last.
    q.push_str(esc);
    q.push_str("[6n");
    q.push_str(end);
    q
}

/// Parse whatever came back.
///
/// Deliberately forgiving about *order* and about junk in between: replies
/// interleave with keyboard input and with answers to queries `crossterm` made
/// during its own startup, so this scans for each shape independently rather
/// than expecting a fixed sequence. Anything unrecognised is ignored.
pub fn parse(reply: &str) -> TermCaps {
    let mut caps = TermCaps {
        answered: !reply.is_empty(),
        ..TermCaps::default()
    };

    // Kitty: `ESC _ G i=<id>;OK ESC \`. Accept only our own id, so a reply to
    // some other query — the shared-memory probe's, say — cannot be misread.
    let needle = format!("_Gi={KITTY_QUERY_ID};");
    if let Some(at) = reply.find(&needle) {
        caps.kitty = reply[at + needle.len()..].starts_with("OK");
    }

    // Primary DA: `ESC [ ? <p> ; <p> ; ... c`. Sixel is the parameter `4`, and
    // it must be matched as a whole parameter — `?64;4c` says Sixel, `?64;22c`
    // does not, and a substring search for "4" says yes to both.
    if let Some(params) = csi_params(reply, '?', 'c') {
        caps.sixel = params.iter().any(|p| p == "4");
    }

    // `CSI 16 t` answers `ESC [ 6 ; <height> ; <width> t`.
    if let Some(p) = csi_params(reply, '6', 't') {
        if let [_, h, w] = p.as_slice() {
            caps.cell = cell_from(w, h);
        }
    }
    // Failing that, derive it: window pixels (`CSI 14 t` → `ESC [ 4 ; h ; w t`)
    // divided by window cells (`CSI 18 t` → `ESC [ 8 ; rows ; cols t`).
    if caps.cell.is_none() {
        if let (Some(px), Some(cells)) =
            (csi_params(reply, '4', 't'), csi_params(reply, '8', 't'))
        {
            if let ([_, ph, pw], [_, rows, cols]) = (px.as_slice(), cells.as_slice()) {
                caps.cell = derive_cell(pw, ph, cols, rows);
            }
        }
    }
    caps
}

/// Find `ESC [ <lead> ... <final>` and return its parameters, `<lead>` first.
///
/// Scans for every `ESC [` in turn rather than the first, because replies
/// arrive interleaved and the one we want is rarely alone.
fn csi_params(reply: &str, lead: char, final_byte: char) -> Option<Vec<String>> {
    let bytes: Vec<char> = reply.chars().collect();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] != '\x1b' || bytes[i + 1] != '[' || bytes[i + 2] != lead {
            i += 1;
            continue;
        }
        // Body runs from the lead byte to the final byte, exclusive.
        let start = i + 2;
        let mut j = start;
        while j < bytes.len() && bytes[j] != final_byte && bytes[j] != '\x1b' {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == final_byte {
            let body: String = bytes[start..j].iter().collect();
            // Drop the private marker so `?64;4` and `64;4` parse alike.
            let body = body.strip_prefix('?').unwrap_or(&body).to_string();
            return Some(body.split(';').map(str::to_string).collect());
        }
        i += 1;
    }
    None
}

/// A cell size is only usable if both axes parse and are non-zero.
fn cell_from(w: &str, h: &str) -> Option<(u16, u16)> {
    let (w, h) = (w.parse::<u16>().ok()?, h.parse::<u16>().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Window pixels ÷ window cells, when the terminal answered both.
///
/// Guarded on every axis: a terminal that reports a window size but zero cells
/// (or zero pixels — VTE reports `xpixel=0` through `TIOCGWINSZ`, so a zero here
/// is not hypothetical) must yield `None` and let the caller keep its default,
/// never a divide by zero or a 0×0 cell.
fn derive_cell(pw: &str, ph: &str, cols: &str, rows: &str) -> Option<(u16, u16)> {
    let pw = pw.parse::<u32>().ok()?;
    let ph = ph.parse::<u32>().ok()?;
    let cols = cols.parse::<u32>().ok()?;
    let rows = rows.parse::<u32>().ok()?;
    if pw == 0 || ph == 0 || cols == 0 || rows == 0 {
        return None;
    }
    let (w, h) = (pw / cols, ph / rows);
    (w > 0 && h > 0)
        .then_some((w.min(u16::MAX as u32) as u16, h.min(u16::MAX as u32) as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real VTE 0.84.1 (Ptyxis 50.1, Fedora 44), each query
    /// asked in isolation with a `CSI 5 n` terminator:
    ///
    /// ```text
    /// DA1        -> <ESC>[?61;1;21;22;28c
    /// CSI 16t    -> (silence)
    /// CSI 14t    -> <ESC>[4;700;1008t
    /// CSI 18t    -> <ESC>[8;35;112t
    /// kitty      -> (silence)
    /// XTVERSION  -> <ESC>P>|VTE(8401)<ESC>\
    /// ```
    const VTE_0841: &str = "\x1b[?61;1;21;22;28c\x1b[4;700;1008t\x1b[8;35;112t\x1b[12;1R";

    #[test]
    fn vte_advertises_no_graphics_protocol_at_all() {
        let caps = parse(VTE_0841);
        assert!(caps.answered);
        assert!(!caps.kitty, "VTE speaks no kitty graphics");
        assert!(
            !caps.sixel,
            "Fedora's VTE is built without sixel: DA1 is ?61;1;21;22;28 and has no 4"
        );
        assert_eq!(caps.best(), None);
    }

    /// The bug this module exists to fix: VTE never answers `CSI 16 t`, and
    /// `ratatui-image` throws the whole capability answer away when that
    /// happens. The window-size pair recovers the real cell — 1008/112 = 9 wide,
    /// 700/35 = 20 high — instead of the fabricated 10×20.
    #[test]
    fn a_cell_size_is_recovered_when_csi_16t_is_unanswered() {
        assert_eq!(parse(VTE_0841).cell, Some((9, 20)));
    }

    #[test]
    fn csi_16t_wins_when_the_terminal_does_answer_it() {
        // `CSI 16 t` says 7×14; the window pair would say 9×20. The direct
        // answer is authoritative.
        let reply = "\x1b[6;14;7t\x1b[4;700;1008t\x1b[8;35;112t\x1b[1;1R";
        assert_eq!(parse(reply).cell, Some((7, 14)));
    }

    #[test]
    fn a_terminal_that_advertises_sixel_is_chosen_over_halfblocks() {
        // xterm -ti vt340 style reply.
        let caps = parse("\x1b[?64;1;2;4;6;9;15;22;29c\x1b[6;20;10t\x1b[1;1R");
        assert!(caps.sixel);
        assert_eq!(caps.best(), Some(Graphics::Sixel));
    }

    /// The whole point of parsing DA parameters instead of searching for a
    /// character: `22` and `14` both contain a `4`.
    #[test]
    fn sixel_is_a_whole_da_parameter_not_a_substring() {
        assert!(!parse("\x1b[?62;22;14c\x1b[1;1R").sixel);
        assert!(parse("\x1b[?62;22;4;14c\x1b[1;1R").sixel);
    }

    #[test]
    fn kitty_outranks_sixel_when_both_are_advertised() {
        let reply =
            format!("\x1b_Gi={KITTY_QUERY_ID};OK\x1b\\\x1b[?64;4c\x1b[6;14;7t\x1b[1;1R");
        let caps = parse(&reply);
        assert!(caps.kitty && caps.sixel);
        assert_eq!(caps.best(), Some(Graphics::Kitty));
    }

    /// A kitty reply carrying somebody else's image id is not our answer. The
    /// shared-memory handshake runs in the same round trip and uses its own id.
    #[test]
    fn a_kitty_reply_for_another_id_is_not_our_answer() {
        assert!(!parse("\x1b_Gi=6710281;OK\x1b\\\x1b[1;1R").kitty);
        // ...and an error reply for our own id is a no, not a yes.
        let refused = format!("\x1b_Gi={KITTY_QUERY_ID};ENOTSUPPORTED\x1b\\\x1b[1;1R");
        assert!(!parse(&refused).kitty);
    }

    /// Silence must be distinguishable from "answered, supports nothing" — the
    /// caller keeps whatever it already had rather than forcing half-blocks on a
    /// terminal it simply failed to reach.
    #[test]
    fn silence_is_not_an_answer() {
        let caps = parse("");
        assert!(!caps.answered);
        assert_eq!(caps.best(), None);
        assert_eq!(caps.cell, None);
    }

    #[test]
    fn garbage_between_replies_is_ignored() {
        let reply = "keypress\x1b[Agarbage\x1b[?64;4c\x1b[6;14;7t\x1b[1;1R";
        let caps = parse(reply);
        assert!(caps.sixel);
        assert_eq!(caps.cell, Some((7, 14)));
    }

    /// A zero anywhere in the window-size pair must not divide by zero or
    /// produce a 0-pixel cell.
    #[test]
    fn a_degenerate_window_report_yields_no_cell() {
        assert_eq!(parse("\x1b[4;0;0t\x1b[8;35;112t\x1b[1;1R").cell, None);
        assert_eq!(parse("\x1b[4;700;1008t\x1b[8;0;0t\x1b[1;1R").cell, None);
        // A window smaller than one cell per column rounds to zero, not to a
        // cell of width 0.
        assert_eq!(parse("\x1b[4;10;10t\x1b[8;35;112t\x1b[1;1R").cell, None);
    }

    #[test]
    fn the_query_ends_with_the_fence_so_silence_is_conclusive() {
        let q = query(false);
        assert!(
            q.ends_with("\x1b[6n"),
            "the cursor report must be last: {q:?}"
        );
        assert!(q.contains("\x1b[c"), "primary DA is how sixel is detected");
        assert!(q.contains("a=q"), "kitty is asked, never inferred");
        assert!(
            q.contains("\x1b[16t") && q.contains("\x1b[14t") && q.contains("\x1b[18t")
        );
    }

    #[test]
    fn the_tmux_query_is_wrapped_and_double_escaped() {
        let q = query(true);
        assert!(q.starts_with("\x1bPtmux;"));
        assert!(q.ends_with("\x1b\\"));
        assert!(q.contains("\x1b\x1b[c"));
    }

    /// The ladder itself, pinned: half-blocks is the *last* resort, not the
    /// default for anything that is not kitty.
    #[test]
    fn the_preference_order_is_kitty_sixel_iterm2_halfblocks() {
        let mut order = [
            Graphics::Halfblocks,
            Graphics::Iterm2,
            Graphics::Kitty,
            Graphics::Sixel,
        ];
        order.sort();
        assert_eq!(
            order,
            [
                Graphics::Kitty,
                Graphics::Sixel,
                Graphics::Iterm2,
                Graphics::Halfblocks
            ]
        );
    }
}

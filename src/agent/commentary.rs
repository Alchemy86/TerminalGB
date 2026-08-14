//! The commentary panel's data — the part that makes this a stream.
//!
//! # Who this is written for
//!
//! A stranger who arrived on Twitch nine seconds ago, on a phone, with no
//! context, who cannot pause and cannot zoom. That reader sets every rule here:
//!
//! * **Current goal first, history second.** A scrolling log is unreadable at a
//!   glance and worthless to someone joining cold — by the time they have read
//!   enough lines to work out what is going on, the lines have moved. So the
//!   panel has a standing [`Commentary::goal`] ("get out of the house and find
//!   Professor Oak") that changes rarely, a [`Commentary::doing`] line that
//!   changes often, and only a short [`Commentary::history`] behind them.
//! * **Few words, plain English.** Not raw model output, not JSON, not
//!   `{"do":"walk"}`. Every line is capped at [`MAX_LINE`] characters so it
//!   cannot wrap into a wall of text at phone size.
//! * **Nothing that could embarrass anyone if clipped.** The stream is public
//!   and permanent: whatever the panel renders can be screenshotted, clipped
//!   and reposted forever. See below.
//!
//! # Everything shown is treated as public
//!
//! Two independent defences, because one is not enough for something that
//! cannot be taken back:
//!
//! 1. **Internal detail never enters the panel in the first place.** Driver
//!    errors reach the panel only as a *category*
//!    ([`Commentary::note_problem`]); the text of an error, a command line, a
//!    file path or a subprocess's stderr goes to the log file, never here.
//! 2. **Everything that does enter is scrubbed** by [`sanitise`], which strips
//!    anything shaped like a credential, a path, a URL, an email address or an
//!    IP address, and drops the line entirely if what is left is not readable
//!    prose. It is a denylist, so it is not a proof — which is why rule 1 does
//!    the real work and this is the net underneath it.

use std::collections::VecDeque;

/// Longest single line the panel will show. Sized so that a line still fits on
/// two rows of a phone-sized viewport after Twitch's downscale.
pub const MAX_LINE: usize = 180;
/// Longest goal line — this one is read first and must never wrap far.
pub const MAX_GOAL: usize = 110;
/// How much history is kept. Enough to give a cold joiner the shape of the last
/// minute, short enough to read.
pub const HISTORY: usize = 40;

/// What a line is, which is how the panel colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The agent's reasoning, in its own words.
    Thought,
    /// A new goal.
    Goal,
    /// Something the executor did.
    Act,
    /// Something the game did.
    Event,
    /// The host talking: paused, budget, resumed.
    System,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Thought => "···",
            Kind::Goal => "GOAL",
            Kind::Act => "▸",
            Kind::Event => "!",
            Kind::System => "sys",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub frame: u64,
    pub kind: Kind,
    pub text: String,
}

/// What the agent is doing right now, in one word for the panel's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Activity {
    #[default]
    Starting,
    /// Waiting for the model.
    Thinking,
    /// Carrying out a plan locally.
    Playing,
    /// Stopped on purpose: budget spent, or the circuit breaker is open.
    Paused,
    /// No driver attached — a human has the controller.
    Off,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Activity::Starting => "starting up",
            Activity::Thinking => "thinking",
            Activity::Playing => "playing",
            Activity::Paused => "paused",
            Activity::Off => "watching",
        }
    }
}

/// The panel's model.
#[derive(Debug, Clone)]
pub struct Commentary {
    goal: String,
    doing: String,
    activity: Activity,
    /// One short line explaining a pause, e.g. "call budget reached". Never an
    /// error message.
    pause_reason: Option<String>,
    history: VecDeque<Entry>,
    /// Total lines ever accepted, so a test can prove a rejected line was
    /// rejected rather than merely absent.
    accepted: u64,
    rejected: u64,
}

impl Default for Commentary {
    fn default() -> Self {
        Commentary {
            goal: "Working out what to do".to_string(),
            doing: String::new(),
            activity: Activity::Starting,
            pause_reason: None,
            history: VecDeque::new(),
            accepted: 0,
            rejected: 0,
        }
    }
}

impl Commentary {
    pub fn goal(&self) -> &str {
        &self.goal
    }

    pub fn doing(&self) -> &str {
        &self.doing
    }

    pub fn activity(&self) -> Activity {
        self.activity
    }

    pub fn pause_reason(&self) -> Option<&str> {
        self.pause_reason.as_deref()
    }

    /// Newest last.
    pub fn history(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.history.iter()
    }

    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    pub fn set_activity(&mut self, a: Activity) {
        self.activity = a;
        if a != Activity::Paused {
            self.pause_reason = None;
        }
    }

    /// Set the standing goal. Ignored if the text does not survive
    /// [`sanitise`] or is the same as the current goal.
    pub fn set_goal(&mut self, frame: u64, text: &str) {
        let Some(clean) = sanitise(text, MAX_GOAL) else {
            self.rejected += 1;
            return;
        };
        if clean == self.goal {
            return;
        }
        self.goal = clean.clone();
        self.push(frame, Kind::Goal, &clean);
    }

    /// Set the "right now" line. Not pushed to history — it changes far too
    /// often to be worth keeping, and history is for reading.
    pub fn set_doing(&mut self, text: &str) {
        match sanitise(text, MAX_LINE) {
            Some(clean) => self.doing = clean,
            None => self.rejected += 1,
        }
    }

    /// Add a line of the agent's own reasoning.
    pub fn thought(&mut self, frame: u64, text: &str) {
        self.add(frame, Kind::Thought, text);
    }

    /// Add a line about what was actually done.
    pub fn act(&mut self, frame: u64, text: &str) {
        self.add(frame, Kind::Act, text);
    }

    /// Add a line about something the game did.
    pub fn event(&mut self, frame: u64, text: &str) {
        self.add(frame, Kind::Event, text);
    }

    /// The host talking. Callers must pass text they wrote themselves — this
    /// is still sanitised, but it is not a channel for error strings.
    pub fn system(&mut self, frame: u64, text: &str) {
        self.add(frame, Kind::System, text);
    }

    /// Report that the driver had a problem, **by category**.
    ///
    /// This is the whole safety story for errors: the panel is told "the agent
    /// timed out", never the exception, the command line, the path or the
    /// stderr. Those go to the log file.
    pub fn note_problem(&mut self, frame: u64, category: Problem) {
        self.add(frame, Kind::System, category.public_text());
    }

    /// Stop, with a short public reason.
    pub fn pause(&mut self, frame: u64, reason: &str) {
        self.activity = Activity::Paused;
        let clean = sanitise(reason, MAX_GOAL).unwrap_or_else(|| "stopped".to_string());
        self.pause_reason = Some(clean.clone());
        self.add(frame, Kind::System, &format!("Paused — {clean}"));
    }

    fn add(&mut self, frame: u64, kind: Kind, text: &str) {
        match sanitise(text, MAX_LINE) {
            Some(clean) => self.push(frame, kind, &clean),
            None => self.rejected += 1,
        }
    }

    fn push(&mut self, frame: u64, kind: Kind, clean: &str) {
        // Do not repeat the previous line verbatim — an agent stuck in a loop
        // would otherwise fill the whole panel with one sentence.
        if let Some(last) = self.history.back() {
            if last.kind == kind && last.text == clean {
                return;
            }
        }
        self.history.push_back(Entry {
            frame,
            kind,
            text: clean.to_string(),
        });
        self.accepted += 1;
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }
}

/// A category of driver failure that may be shown publicly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Timeout,
    Unreadable,
    Crashed,
    NoActions,
}

impl Problem {
    /// The **only** text a viewer ever sees for a failure.
    pub fn public_text(self) -> &'static str {
        match self {
            Problem::Timeout => "The agent took too long to answer — trying again.",
            Problem::Unreadable => "The agent's answer didn't make sense — trying again.",
            Problem::Crashed => "The agent hit a problem — trying again.",
            Problem::NoActions => "The agent didn't say what to do — asking again.",
        }
    }

    /// Short label for a counter in the status line.
    pub fn label(self) -> &'static str {
        match self {
            Problem::Timeout => "timeout",
            Problem::Unreadable => "unreadable",
            Problem::Crashed => "error",
            Problem::NoActions => "empty",
        }
    }
}

/// Make a line safe to broadcast, or reject it.
///
/// Returns `None` when what is left is not worth showing — empty, or so heavily
/// redacted that it is noise. **Rejecting is always allowed**: a missing line
/// costs a viewer nothing, a leaked one cannot be taken back.
pub fn sanitise(input: &str, max: usize) -> Option<String> {
    // 1. Strip ANSI escapes and control characters outright. A terminal panel
    //    renders escapes, so an agent (or a compromised subprocess) that emits
    //    them could repaint the screen.
    let mut stripped = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Drop the whole escape sequence: everything up to a letter.
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        if c == '\n' || c == '\t' || c == '\r' {
            stripped.push(' ');
        } else if (c as u32) < 0x20 || c == '\u{7f}' {
            continue;
        } else if matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{feff}')
        {
            // Zero-width and bidirectional-override characters: invisible on
            // screen, so they can hide text or reverse it.
            continue;
        } else {
            stripped.push(c);
        }
    }

    // 2. Redact word by word. Word-level is the right granularity: a secret is
    //    a token, and replacing the token keeps the sentence readable.
    let mut redacted = false;
    let mut words: Vec<String> = Vec::new();
    for word in stripped.split_whitespace() {
        if looks_private(word) {
            redacted = true;
            if words.last().map(|w| w != "[redacted]").unwrap_or(true) {
                words.push("[redacted]".to_string());
            }
        } else {
            words.push(word.to_string());
        }
    }
    let mut out = words.join(" ");

    // 3. Nothing left, or nothing but redactions: not worth showing.
    let visible = out.replace("[redacted]", "");
    if visible.trim().is_empty() {
        return None;
    }
    // 4. Prose check. Model output that is mostly punctuation or symbols is
    //    almost always a serialised structure that leaked into the wrong field.
    let letters = visible.chars().filter(|c| c.is_alphanumeric()).count();
    if letters * 2 < visible.chars().filter(|c| !c.is_whitespace()).count() {
        return None;
    }
    if redacted && letters < 8 {
        return None;
    }

    // 5. Length. Cut on a word boundary so the last word is not half a word.
    if out.chars().count() > max {
        let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
        if let Some(sp) = cut.rfind(' ') {
            if sp > max / 2 {
                cut.truncate(sp);
            }
        }
        cut.push('…');
        out = cut;
    }
    Some(out)
}

/// Does this word look like something that must not be broadcast?
fn looks_private(word: &str) -> bool {
    let w = word.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '(' | ')' | '[' | ']' | ',' | '.' | ';' | ':'
        )
    });
    if w.is_empty() {
        return false;
    }
    let lower = w.to_ascii_lowercase();

    // Anything shaped like an API key.
    if lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("pk-")
        || lower.starts_with("ghp_")
        || lower.starts_with("gho_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xox")
        || lower.starts_with("bearer")
        || lower.starts_with("aws_")
        || lower.starts_with("akia")
    {
        return true;
    }
    // Environment variables that carry credentials, and their assignments.
    if lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("api-key")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("token=")
        || lower.contains("auth=")
    {
        return true;
    }
    // Absolute paths, home paths and Windows paths.
    if w.starts_with('/') && w[1..].contains('/') {
        return true;
    }
    if w.starts_with("~/") || lower.starts_with("c:\\") || w.contains("\\Users\\") {
        return true;
    }
    // Anything with a path separator and a file extension, e.g.
    // `src/agent/mod.rs` — internal structure, of no interest to a viewer.
    if (w.contains('/') || w.contains('\\'))
        && w.rsplit('.')
            .next()
            .map(|e| {
                e.len() <= 4
                    && !e.is_empty()
                    && e.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .unwrap_or(false)
    {
        return true;
    }
    // URLs.
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ws://")
        || lower.starts_with("wss://")
        || lower.starts_with("file://")
    {
        return true;
    }
    // Email addresses.
    if w.contains('@') && w.contains('.') && !w.starts_with('@') {
        return true;
    }
    // Bare IPv4 addresses (possibly with a port).
    let host = lower.split(':').next().unwrap_or("");
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() == 4
        && octets
            .iter()
            .all(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_digit()))
    {
        return true;
    }
    // A long opaque token: 24+ characters of base64/hex alphabet with no
    // vowels-and-spaces structure. Catches keys whose prefix we do not know.
    if w.len() >= 24
        && w.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c == '_'
                || c == '-'
                || c == '+'
                || c == '/'
                || c == '='
        })
        && w.chars().filter(|c| c.is_ascii_digit()).count() >= 4
        && w.chars().filter(|c| c.is_ascii_uppercase()).count() >= 3
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that cannot be got wrong: nothing private reaches the panel.
    /// Each of these is a real shape that has appeared in model output or in an
    /// error message somewhere.
    #[test]
    fn credentials_paths_and_urls_never_reach_the_panel() {
        let cases = [
            "my key is sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF and it works",
            "set ANTHROPIC_API_KEY=abcdef123456 before running",
            "failed to open /home/example-user/.claude/.credentials.json for reading",
            "error at C:\\Users\\example-user\\secret.txt while starting",
            "posting the state to https://api.anthropic.com/v1/messages now",
            "the adapter lives in tools/claude_plays/adapter.py somewhere",
            "reaching the box at 192.168.1.44:8080 to continue the run",
            "mailing the log to someone@example.com right now",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            "the token=9f8e7d6c5b4a3f2e1d0c9b8a7 was rejected by the server",
        ];
        for c in cases {
            let out = sanitise(c, MAX_LINE);
            let text = out.unwrap_or_default();
            for needle in [
                "sk-ant",
                "ANTHROPIC_API_KEY",
                "/home/example-user",
                "C:\\Users",
                "https://",
                "adapter.py",
                "192.168.1.44",
                "@example.com",
                "eyJhbGciOi",
                "9f8e7d6c5b4a3f2e1d0c9b8a7",
            ] {
                assert!(
                    !text.contains(needle),
                    "sanitise leaked {needle:?} from {c:?} -> {text:?}"
                );
            }
        }
    }

    #[test]
    fn ordinary_commentary_survives_intact() {
        for s in [
            "I'm in the bedroom. The stairs are to the left, so I'll head there.",
            "That didn't work — a wall. Trying the other way round.",
            "Oak is talking about Pokemon. Reading through it.",
            "Picked Squirtle! Water beats Brock's rock types later on.",
        ] {
            assert_eq!(sanitise(s, MAX_LINE).as_deref(), Some(s));
        }
    }

    #[test]
    fn escapes_and_invisible_characters_are_removed() {
        let nasty = "hello \u{1b}[2J\u{1b}[H there\u{200b}\u{202e}friend and everyone";
        let out = sanitise(nasty, MAX_LINE).unwrap();
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert!(!out.contains('\u{200b}'), "{out:?}");
        assert!(!out.contains('\u{202e}'), "{out:?}");
        assert!(out.contains("hello"), "{out:?}");
        assert!(out.contains("friend"), "{out:?}");
    }

    #[test]
    fn json_and_symbol_soup_is_rejected_not_shown() {
        assert_eq!(sanitise(r#"{"do":"walk","dir":"<<<>>>"}"#, MAX_LINE), None);
        assert_eq!(sanitise("   ", MAX_LINE), None);
        assert_eq!(sanitise("###!!!***", MAX_LINE), None);
    }

    #[test]
    fn long_lines_are_cut_at_a_word_boundary() {
        let long = "word ".repeat(200);
        let out = sanitise(&long, 60).unwrap();
        assert!(out.chars().count() <= 60, "{}", out.chars().count());
        assert!(out.ends_with('…'));
        assert!(!out.contains("wor…"), "cut mid-word: {out:?}");
    }

    #[test]
    fn errors_reach_the_panel_only_as_a_category() {
        let mut c = Commentary::default();
        c.note_problem(1, Problem::Timeout);
        let last = c.history().next_back().unwrap();
        assert_eq!(last.text, Problem::Timeout.public_text());
        assert!(!last.text.contains("sk-"));
        // And the categories themselves are viewer-facing English.
        for p in [
            Problem::Timeout,
            Problem::Unreadable,
            Problem::Crashed,
            Problem::NoActions,
        ] {
            assert!(sanitise(p.public_text(), MAX_LINE).is_some());
        }
    }

    #[test]
    fn the_goal_is_the_standing_line_and_repeats_are_not_re_logged() {
        let mut c = Commentary::default();
        c.set_goal(10, "Get out of the house");
        assert_eq!(c.goal(), "Get out of the house");
        assert_eq!(c.history().count(), 1);
        c.set_goal(20, "Get out of the house");
        assert_eq!(c.history().count(), 1, "an unchanged goal is not re-logged");
        c.set_goal(30, "Find Professor Oak");
        assert_eq!(c.goal(), "Find Professor Oak");
        assert_eq!(c.history().count(), 2);
    }

    #[test]
    fn a_stuck_agent_cannot_fill_the_panel_with_one_sentence() {
        let mut c = Commentary::default();
        for f in 0..50 {
            c.thought(f, "Trying the door again.");
        }
        assert_eq!(c.history().count(), 1);
        assert_eq!(c.accepted(), 1);
    }

    #[test]
    fn history_is_bounded() {
        let mut c = Commentary::default();
        for f in 0..(HISTORY as u64 * 3) {
            c.thought(f, &format!("step {f}"));
        }
        assert_eq!(c.history().count(), HISTORY);
        assert_eq!(c.history().next_back().unwrap().text, "step 119");
    }

    #[test]
    fn a_rejected_line_is_counted_so_it_can_be_noticed() {
        let mut c = Commentary::default();
        c.thought(1, "{}{}{}");
        assert_eq!(c.history().count(), 0);
        assert_eq!(c.rejected(), 1);
    }
}

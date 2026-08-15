//! The seam an external agent — a language model, a policy network, a person —
//! plugs into.
//!
//! # The shape, and why it is a subprocess
//!
//! An external agent is a **child process**: it is handed one JSON object per
//! decision on stdin and must answer with one JSON object on stdout. That is the
//! same contract `plugins/agent`'s `CommandDriver` already uses in this
//! repository, and it is chosen for the same three reasons.
//!
//! * It adds **no dependency**. This crate has none and must keep none, so an
//!   HTTP client is out of the question; a subprocess needs only `std`.
//! * It is **language-agnostic**. A Python script, a shell one-liner and a
//!   compiled binary are all the same thing from here.
//! * It keeps the network **out of the engine**. The adapter decides whether to
//!   call an API, a local model or nothing at all, and the engine never learns
//!   about credentials, retries or rate limits.
//!
//! # The protocol
//!
//! One request per line in, one reply per line out.
//!
//! ```json
//! {"schema":"gb1battle/1","turn":3,"side":"player",
//!  "me":{"species":25,"name":"PIKACHU","level":20,"hp":41,"max_hp":60,
//!        "status":"OK","types":["ELECTRIC"],
//!        "stages":{"attack":0,"defense":0,"speed":0,"special":0},
//!        "moves":[{"slot":0,"id":85,"name":"THUNDERBOLT","power":95,
//!                  "type":"ELECTRIC","accuracy":100,"pp":15,
//!                  "expected_damage":54,"effectiveness":200}]},
//!  "them":{...},
//!  "legal":[{"kind":"move","slot":0},{"kind":"switch","slot":1}]}
//! ```
//!
//! and back:
//!
//! ```json
//! {"kind":"move","slot":0}
//! ```
//!
//! # What the adapter is deliberately NOT given
//!
//! No random bytes, no seed, and no hidden member of the opposing party. An
//! adapter that could see the dice would win by manipulation and its score would
//! be worthless — the honesty rules in [`crate::rng`]. `expected_damage` **is**
//! supplied, because it is derivable from what a player can see and withholding
//! it would measure arithmetic rather than judgement.
//!
//! # Failure is not a free pass
//!
//! A child that crashes, times out, or answers with something illegal does not
//! get its turn replayed. The first legal action is substituted and the episode
//! records it ([`crate::eval::EpisodeResult::illegal_choices`],
//! [`ExternalAgent::failures`]), because an agent that fails half its turns and
//! is quietly rescued is being scored on the harness's play, not its own.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use super::{expected_damage, Agent, View};
use crate::mechanics as mech;
use crate::state::{Action, BattleMon, Side};

/// The protocol version. Bump it if the request or reply shape changes, so an
/// adapter written against an older one fails loudly rather than subtly.
pub const SCHEMA: &str = "gb1battle/1";

/// An agent that lives in another process.
pub struct ExternalAgent {
    name: String,
    program: String,
    args: Vec<String>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    timeout: Duration,
    failures: u32,
    calls: u32,
    total_wait: Duration,
}

impl ExternalAgent {
    /// Spawn `program args…`, speaking the protocol on its stdin and stdout.
    ///
    /// The child is spawned lazily, on the first decision, so constructing an
    /// agent that is never used costs nothing.
    pub fn new(
        name: impl Into<String>,
        program: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        ExternalAgent {
            name: name.into(),
            program: program.into(),
            args,
            child: None,
            stdin: None,
            stdout: None,
            timeout: Duration::from_secs(30),
            failures: 0,
            calls: 0,
            total_wait: Duration::ZERO,
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// How many decisions the child failed to answer usably. Printed with the
    /// result; a high count invalidates it.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    pub fn calls(&self) -> u32 {
        self.calls
    }

    /// Mean wall-clock time per decision — what a broadcast viewer experiences
    /// as think-time, and the cost that has to be quoted beside a win rate.
    pub fn mean_latency(&self) -> Duration {
        if self.calls == 0 {
            Duration::ZERO
        } else {
            self.total_wait / self.calls
        }
    }

    fn ensure(&mut self) -> Result<(), String> {
        if self.child.is_some() {
            return Ok(());
        }
        let mut c = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start `{}`: {e}", self.program))?;
        self.stdin = c.stdin.take();
        self.stdout = c.stdout.take().map(BufReader::new);
        self.child = Some(c);
        Ok(())
    }

    fn ask(&mut self, request: &str) -> Result<String, String> {
        self.ensure()?;
        let started = Instant::now();
        {
            let w = self.stdin.as_mut().ok_or("no stdin")?;
            writeln!(w, "{request}").map_err(|e| e.to_string())?;
            w.flush().map_err(|e| e.to_string())?;
        }
        let mut line = String::new();
        let r = self.stdout.as_mut().ok_or("no stdout")?;
        // A blocking read with no timeout would hang the whole evaluation on one
        // wedged child, so the deadline is enforced by the caller's process
        // supervision; here the read is bounded by the child closing its pipe.
        // (`read_line` returning Ok(0) is EOF, which is a dead child.)
        let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
        self.total_wait += started.elapsed();
        if n == 0 {
            self.child = None;
            self.stdin = None;
            self.stdout = None;
            return Err("the agent process closed its output".into());
        }
        if started.elapsed() > self.timeout {
            return Err(format!(
                "the agent took {:?}, over its budget",
                started.elapsed()
            ));
        }
        Ok(line)
    }
}

impl Drop for ExternalAgent {
    fn drop(&mut self) {
        // Close the pipe first so a well-behaved child sees EOF and exits, then
        // make sure. An adapter left running would hold a model session open.
        self.stdin.take();
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Agent for ExternalAgent {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn describe(&self) -> String {
        format!(
            "external({} {}) — {} call(s), {} failure(s), {:.0}ms mean",
            self.program,
            self.args.join(" "),
            self.calls,
            self.failures,
            self.mean_latency().as_secs_f32() * 1000.0
        )
    }

    fn choose(&mut self, view: &View) -> Action {
        self.calls += 1;
        let request = encode_request(view);
        match self.ask(&request).and_then(|r| decode_reply(&r)) {
            Ok(a) => a,
            Err(_) => {
                // The failure is counted and the caller substitutes a legal
                // action. It is deliberately NOT retried: an agent that gets a
                // second attempt at every hard turn is being scored on a
                // different game.
                self.failures += 1;
                view.legal.first().copied().unwrap_or(Action::Struggle)
            }
        }
    }
}

/// Build the JSON a decision is offered as.
///
/// Hand-rolled, because this crate has no dependencies and a decision object is
/// a dozen fields. Strings that reach it are species and move names read out of
/// the cartridge, so they are escaped rather than trusted.
pub fn encode_request(view: &View) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{{\"schema\":\"{SCHEMA}\",\"turn\":{},\"side\":\"{}\",",
        view.battle.turn,
        view.side.name()
    ));
    s.push_str("\"me\":");
    encode_mon(&mut s, view, view.side, true);
    s.push_str(",\"them\":");
    encode_mon(&mut s, view, view.side.other(), false);
    s.push_str(",\"party\":{\"mine_alive\":");
    s.push_str(&view.battle.side(view.side).alive().to_string());
    s.push_str(",\"theirs_alive\":");
    s.push_str(&view.battle.side(view.side.other()).alive().to_string());
    s.push('}');
    s.push_str(",\"legal\":[");
    for (i, a) in view.legal.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match a {
            Action::Move(slot) => {
                s.push_str(&format!("{{\"kind\":\"move\",\"slot\":{slot}}}"))
            }
            Action::Switch(slot) => {
                s.push_str(&format!("{{\"kind\":\"switch\",\"slot\":{slot}}}"))
            }
            Action::Run => s.push_str("{\"kind\":\"run\"}"),
            Action::Struggle => s.push_str("{\"kind\":\"struggle\"}"),
        }
    }
    s.push_str("]}");
    s
}

fn encode_mon(s: &mut String, view: &View, side: Side, mine: bool) {
    let mon = view.battle.active(side);
    let name = view
        .data
        .species_name(mon.species)
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", mon.species));
    s.push_str(&format!(
        "{{\"species\":{},\"name\":\"{}\",\"level\":{},\"hp\":{},\"max_hp\":{},\"status\":\"{}\"",
        mon.species,
        escape(&name),
        mon.level,
        mon.hp,
        mon.max_hp,
        mon.status.label()
    ));
    s.push_str(&format!(
        ",\"types\":[\"{}\"{}]",
        mon.type1.debug_name(),
        if mon.type2 == mon.type1 {
            String::new()
        } else {
            format!(",\"{}\"", mon.type2.debug_name())
        }
    ));
    // Stages are reported the way a PLAYER reads them: −6..+6, not the stored
    // 1..13. An adapter that had to know about the internal offset would be
    // being asked to model the cartridge rather than to play it.
    s.push_str(&format!(
        ",\"stages\":{{\"attack\":{},\"defense\":{},\"speed\":{},\"special\":{},\"accuracy\":{},\"evasion\":{}}}",
        i16::from(mon.stages.attack) - 7,
        i16::from(mon.stages.defense) - 7,
        i16::from(mon.stages.speed) - 7,
        i16::from(mon.stages.special) - 7,
        i16::from(mon.stages.accuracy) - 7,
        i16::from(mon.stages.evasion) - 7
    ));
    s.push_str(&format!(
        ",\"stats\":{{\"attack\":{},\"defense\":{},\"speed\":{},\"special\":{}}}",
        mon.current.attack, mon.current.defense, mon.current.speed, mon.current.special
    ));
    if mine {
        s.push_str(",\"moves\":[");
        let them = view.battle.active(side.other());
        let base_speed = mech::base_speed(view.data, mon);
        let mut first = true;
        for (slot, m) in mon.moves.iter().enumerate() {
            if m.is_empty() {
                continue;
            }
            let Some(d) = view.data.move_data(m.id) else {
                continue;
            };
            if !first {
                s.push(',');
            }
            first = false;
            let mv_name = view
                .data
                .move_name(m.id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{}", m.id));
            let eff = view
                .data
                .chart()
                .effectiveness_percent(d.kind, them.types());
            s.push_str(&format!(
                "{{\"slot\":{slot},\"id\":{},\"name\":\"{}\",\"power\":{},\"type\":\"{}\",\
                 \"accuracy\":{},\"pp\":{},\"expected_damage\":{},\"effectiveness\":{}}}",
                m.id,
                escape(&mv_name),
                d.power,
                d.kind.debug_name(),
                // Reported as a percentage a human recognises, not as the
                // cartridge's `n * $FF / 100` byte.
                (u16::from(d.accuracy) * 100 + 127) / 255,
                m.pp,
                expected_damage(mon, them, d, view.data.chart(), base_speed).round() as u32,
                eff
            ));
        }
        s.push(']');
    }
    s.push('}');
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Parse a reply.
///
/// Deliberately forgiving about whitespace and field order and deliberately
/// strict about everything else: an unparseable reply is an error, never a
/// guess. A guessed action would be the harness playing.
pub fn decode_reply(line: &str) -> Result<Action, String> {
    let kind = json_string(line, "kind").ok_or("no `kind` in the reply")?;
    let slot = json_number(line, "slot");
    match kind.as_str() {
        "move" => slot
            .map(|n| Action::Move(n as usize))
            .ok_or_else(|| "a `move` reply needs a `slot`".into()),
        "switch" => slot
            .map(|n| Action::Switch(n as usize))
            .ok_or_else(|| "a `switch` reply needs a `slot`".into()),
        "run" => Ok(Action::Run),
        "struggle" => Ok(Action::Struggle),
        other => Err(format!("unknown action kind `{other}`")),
    }
}

fn json_string(s: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let at = s.find(&pat)? + pat.len();
    let rest = &s[at..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn json_number(s: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\"");
    let at = s.find(&pat)? + pat.len();
    let rest = &s[at..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// A `BattleMon`'s one-line summary, for an adapter that would rather read prose
/// than JSON. Not part of the protocol; a convenience for a human debugging one.
pub fn describe_mon(mon: &BattleMon, data: &crate::data::GameData) -> String {
    let name = data
        .species_name(mon.species)
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{}", mon.species));
    format!(
        "{name} L{} {}/{} {}",
        mon.level,
        mon.hp,
        mon.max_hp,
        mon.status.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Side;
    use crate::testsupport::{battler, duel, engine, switch_both};

    fn a_view_request() -> String {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let v = View::new(&b, e.data(), Side::Player, &legal);
        encode_request(&v)
    }

    #[test]
    fn a_request_carries_the_schema_and_the_legal_actions() {
        let r = a_view_request();
        assert!(r.contains(SCHEMA), "{r}");
        assert!(r.contains("\"legal\":["), "{r}");
        assert!(r.contains("\"kind\":\"move\""), "{r}");
        assert!(r.contains("\"expected_damage\""), "{r}");
    }

    /// **The adapter is never told the dice.** An agent that could see them
    /// would win by manipulation and its score would be worthless.
    #[test]
    fn a_request_never_carries_a_seed_or_a_random_byte() {
        let r = a_view_request();
        for forbidden in ["seed", "rng", "random", "roll"] {
            assert!(!r.contains(forbidden), "`{forbidden}` leaked into: {r}");
        }
    }

    /// Stat stages are reported the way a player reads them, not the way the
    /// cartridge stores them.
    #[test]
    fn stages_are_reported_as_a_player_reads_them() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        // After the switch-in, which resets stages — a stage set before it
        // would be wiped, exactly as the cartridge wipes one.
        switch_both(&mut b);
        b.active_mut(Side::Player).stages.attack = 9; // stored 9 is +2
        let legal = e.legal_actions(&b, Side::Player);
        let r = encode_request(&View::new(&b, e.data(), Side::Player, &legal));
        assert!(r.contains("\"attack\":2"), "{r}");
        assert!(!r.contains("\"attack\":9"), "{r}");
    }

    /// The opponent's moves are not disclosed — a player cannot see them.
    #[test]
    fn the_opponents_moves_are_not_disclosed() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [5, 6, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let r = encode_request(&View::new(&b, e.data(), Side::Player, &legal));
        // "moves" appears once, in the `me` object.
        assert_eq!(r.matches("\"moves\":").count(), 1, "{r}");
    }

    #[test]
    fn replies_decode() {
        assert_eq!(
            decode_reply(r#"{"kind":"move","slot":2}"#),
            Ok(Action::Move(2))
        );
        assert_eq!(
            decode_reply(r#"  { "slot" : 1 , "kind" : "switch" } "#),
            Ok(Action::Switch(1))
        );
        assert_eq!(decode_reply(r#"{"kind":"run"}"#), Ok(Action::Run));
        assert_eq!(decode_reply(r#"{"kind":"struggle"}"#), Ok(Action::Struggle));
    }

    /// An unparseable reply is an error, never a guess.
    #[test]
    fn a_bad_reply_is_an_error_not_a_guess() {
        assert!(decode_reply("").is_err());
        assert!(decode_reply("not json").is_err());
        assert!(decode_reply(r#"{"kind":"move"}"#).is_err());
        assert!(decode_reply(r#"{"kind":"teleport","slot":0}"#).is_err());
    }

    /// A name from the cartridge is escaped, not trusted.
    #[test]
    fn a_name_with_a_quote_in_it_is_escaped() {
        assert_eq!(escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape("a\nb"), "a\\nb");
    }

    /// A missing adapter costs the turn, not the run — and it is counted.
    #[test]
    fn an_adapter_that_cannot_start_fails_the_turn_and_says_so() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = ExternalAgent::new(
            "missing",
            "/nonexistent/definitely-not-a-program",
            vec![],
        );
        let chosen = a.choose(&View::new(&b, e.data(), Side::Player, &legal));
        assert!(legal.contains(&chosen));
        assert_eq!(
            a.failures(),
            1,
            "the failure must be counted, not swallowed"
        );
    }

    /// A real child process round-trips. Uses `sh`, which is present anywhere
    /// this repository builds; if it is not, the test is the wrong thing to fix.
    #[test]
    fn a_real_child_process_answers_a_decision() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        // An adapter that always picks slot 1.
        let mut a = ExternalAgent::new(
            "always-slot-1",
            "sh",
            vec![
                "-c".into(),
                "while read -r _line; do echo '{\"kind\":\"move\",\"slot\":1}'; done"
                    .into(),
            ],
        );
        for _ in 0..3 {
            let chosen = a.choose(&View::new(&b, e.data(), Side::Player, &legal));
            assert_eq!(chosen, Action::Move(1));
        }
        assert_eq!(a.failures(), 0);
        assert_eq!(a.calls(), 3);
    }
}

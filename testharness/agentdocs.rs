//! Gate: `docs/agent-reference.md` must agree with the code it documents.
//!
//! ```sh
//! cargo test --release --features plugin-agent --test agentdocs
//! ```
//!
//! A reference page is only worth reading if a reader can act on it, and the
//! way a reference page fails is **silently**: a constant is tuned, a variant
//! is renamed, a ceiling is raised, and the page keeps saying the old thing
//! with the same confidence. This repository already learned that lesson twice
//! — the audio cost that outlived its fix by months, and the picture-mode cost
//! constant now checked against a live measurement — so the page's numbers and
//! names are checked here rather than trusted.
//!
//! What it checks:
//!
//! * every default in [`Limits`] and [`Budget`] is printed on the page, with
//!   the value the code actually has;
//! * every executor constant (`WALK_HOLD`, `MENU_GAP`, …) likewise;
//! * every variant *name string* a driver can receive — `Mode`, `ScreenKind`,
//!   `Trust`, `Halt` — appears, so a renamed state cannot go undocumented;
//! * every public `Halt` sentence appears **verbatim**, because those are the
//!   words a viewer reads;
//! * every action verb and alias the page advertises really parses, and the
//!   one it says is rejected really is;
//! * the schema string matches;
//! * every tool in the generated `tools/list` capture is named on the page;
//! * the generated examples are present, parse, and carry that same schema;
//! * `docs/agent-environment.md`'s **frame hash and save-state size are
//!   recomputed** from the vendored MIT cartridge, because a hash written into
//!   prose is the single most stale-able thing in this repository.
//!
//! What it deliberately does **not** check, said plainly rather than implied:
//!
//! * **Prose.** A test that pins sentences is a test nobody can edit around,
//!   and the page's job is to explain. The exception is the five `Halt`
//!   sentences, which are broadcast to viewers verbatim.
//! * **A second, stale mention of a value it already found.** A number is
//!   satisfied by *any* line that names its constant, so if the page says
//!   `MENU_GAP` is 24 in one place and 26 in another, this passes. Tightening
//!   that would mean guessing which numbers on a labelled line are values,
//!   which is how a checker starts crying wolf (the same reason
//!   `docs/tools/status_numbers.py` gave up hunting prose for `\d+/\d+`).
//!
//! `harness = false` so a failure prints which claim broke rather than a
//! panic backtrace.

use std::path::{Path, PathBuf};

use terminalgb_plugin_agent::action::{self, Action, Limits};
use terminalgb_plugin_agent::governor::{Budget, Halt};
use terminalgb_plugin_agent::json;
use terminalgb_plugin_agent::observe::{Mode, ScreenKind, Trust};

struct Check {
    what: String,
    ok: bool,
    detail: String,
}

fn root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the crate root whether run by cargo or by hand.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> Option<String> {
    std::fs::read_to_string(root().join(rel)).ok()
}

/// The page must contain `needle`. Reported with a short label so a failure
/// says *which* claim went stale.
fn says(page: &str, what: &str, needle: &str) -> Check {
    Check {
        what: what.to_string(),
        ok: page.contains(needle),
        detail: format!("looked for {needle:?}"),
    }
}

/// Is `n` present in `line` as a whole number — not as a run of digits inside
/// a bigger one, so `24` does not match inside `240` or `0.24`?
fn line_has_number(line: &str, n: u64) -> bool {
    let s = n.to_string();
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(i) = line[from..].find(&s) {
        let at = from + i;
        let bounded = |c: Option<u8>| !matches!(c, Some(b'0'..=b'9') | Some(b'.'));
        if bounded(at.checked_sub(1).map(|j| bytes[j]))
            && bounded(bytes.get(at + s.len()).copied())
        {
            return true;
        }
        from = at + s.len();
    }
    false
}

/// A number, checked **on a line that also names the thing it is the value
/// of**. Scanning the whole page for a bare `24` is far too weak: several of
/// these constants share a value, so a stale one would be covered by a correct
/// occurrence elsewhere. Requiring the number and its label on the same line
/// is what gives this gate teeth — proved by editing one and watching it go
/// red.
fn says_number(page: &str, label: &str, what: &str, n: u64) -> Check {
    let hits: Vec<&str> = page.lines().filter(|l| l.contains(label)).collect();
    Check {
        what: what.to_string(),
        ok: hits.iter().any(|l| line_has_number(l, n)),
        detail: if hits.is_empty() {
            format!("no line on the page mentions {label:?}")
        } else {
            format!(
                "{} line(s) mention {label:?}, none of them the number {n}",
                hits.len()
            )
        },
    }
}

/// The action vocabulary, driven through the real parser rather than described.
fn verb_parses(what: &str, body: &str, want_ok: bool) -> Check {
    let lim = Limits::default();
    let parsed = json::parse(body)
        .ok()
        .map(|v| Action::from_json(&v, &lim))
        .map(|r| r.is_ok())
        .unwrap_or(false);
    Check {
        what: what.to_string(),
        ok: parsed == want_ok,
        detail: format!("{body} parsed={parsed}, wanted {want_ok}"),
    }
}

fn main() {
    let mut checks: Vec<Check> = Vec::new();

    // A published crate ships `src/`, `examples/` and `testharness/` and NOT
    // `docs/` (see `include` in Cargo.toml), so from a crates.io checkout the
    // pages this gate reads are simply absent. That is a loud SKIP, not a
    // failure — the same rule the external-suite scouts follow for a fixture
    // they were not given.
    let Some(page) = read("docs/agent-reference.md") else {
        println!(
            "agentdocs: docs/agent-reference.md is not present — SKIP.\n\
             (Expected outside a full repository checkout: `docs/` is not part \
             of the published crate.)"
        );
        return;
    };

    // -- 1. the numbers the page prints -----------------------------------
    let lim = Limits::default();
    checks.push(says_number(
        &page,
        "plan_frames",
        "Limits::plan_frames",
        lim.plan_frames as u64,
    ));
    checks.push(says_number(
        &page,
        "plan_actions",
        "Limits::plan_actions",
        lim.plan_actions as u64,
    ));
    checks.push(says_number(
        &page,
        "max_repeat",
        "Limits::max_repeat",
        lim.max_repeat as u64,
    ));
    checks.push(says_number(
        &page,
        "max_steps",
        "Limits::max_steps",
        lim.max_steps as u64,
    ));
    checks.push(says_number(
        &page,
        "max_text_presses",
        "Limits::max_text_presses",
        lim.max_text_presses as u64,
    ));
    checks.push(says_number(
        &page,
        "text_settle",
        "Limits::text_settle",
        lim.text_settle as u64,
    ));
    checks.push(says_number(
        &page,
        "max_wait",
        "Limits::max_wait",
        lim.max_wait as u64,
    ));

    checks.push(says_number(
        &page,
        "WALK_HOLD",
        "action::WALK_HOLD",
        action::WALK_HOLD as u64,
    ));
    checks.push(says_number(
        &page,
        "WALK_GAP",
        "action::WALK_GAP",
        action::WALK_GAP as u64,
    ));
    checks.push(says_number(
        &page,
        "WALK_MISSES",
        "action::WALK_MISSES",
        action::WALK_MISSES as u64,
    ));
    checks.push(says_number(
        &page,
        "GOTO_BARREN",
        "action::GOTO_BARREN",
        action::GOTO_BARREN as u64,
    ));
    checks.push(says_number(
        &page,
        "GOTO_DETOUR",
        "action::GOTO_DETOUR",
        action::GOTO_DETOUR as u64,
    ));
    checks.push(says_number(
        &page,
        "MENU_GAP",
        "action::MENU_GAP",
        action::MENU_GAP as u64,
    ));
    checks.push(says_number(
        &page,
        "DEFAULT_HOLD",
        "DEFAULT_HOLD (press_buttons' default hold)",
        terminalgb_plugin_api::DEFAULT_HOLD as u64,
    ));

    let b = Budget::default();
    checks.push(says_number(
        &page,
        "max_calls",
        "Budget::max_calls",
        b.max_calls as u64,
    ));
    // Written for a reader, so 2,000,000 rather than 2000000. Check both.
    let tokens = b.max_tokens;
    let grouped = format!(
        "{},{:03},{:03}",
        tokens / 1_000_000,
        (tokens / 1_000) % 1_000,
        tokens % 1_000
    );
    checks.push(Check {
        what: "Budget::max_tokens".into(),
        ok: page.contains(&grouped) || page.contains(&tokens.to_string()),
        detail: format!("looked for {grouped} or {tokens}"),
    });
    checks.push(says(
        &page,
        "Budget::max_usd",
        &format!("${:.2}", b.max_usd),
    ));
    checks.push(says_number(
        &page,
        "max_consecutive_failures",
        "Budget::max_consecutive_failures",
        b.max_consecutive_failures as u64,
    ));
    checks.push(says_number(
        &page,
        "breaker_cooldown",
        "Budget::breaker_cooldown (seconds)",
        b.breaker_cooldown.as_secs(),
    ));
    checks.push(says_number(
        &page,
        "call_timeout",
        "Budget::call_timeout (seconds)",
        b.call_timeout.as_secs(),
    ));
    checks.push(Check {
        what: "Budget::max_wall is described in hours".into(),
        ok: page.contains(&format!("{} hours", b.max_wall.as_secs() / 3600)),
        detail: format!("{} seconds", b.max_wall.as_secs()),
    });

    // -- 2. every state name a driver can be handed ------------------------
    for m in [
        Mode::Title,
        Mode::Overworld,
        Mode::Dialogue,
        Mode::Menu,
        Mode::Battle,
        Mode::Transition,
        Mode::Cutscene,
    ] {
        checks.push(says(&page, &format!("Mode::{m:?} is documented"), m.name()));
    }
    for k in [
        ScreenKind::NoText,
        ScreenKind::Text,
        ScreenKind::Dialogue,
        ScreenKind::Menu,
        ScreenKind::YesNo,
        ScreenKind::NamingKeyboard,
    ] {
        checks.push(says(
            &page,
            &format!("ScreenKind::{k:?} is documented"),
            k.name(),
        ));
    }
    for t in [Trust::Ok, Trust::Graphics, Trust::Settling, Trust::Off] {
        checks.push(says(
            &page,
            &format!("Trust::{t:?} is documented"),
            t.label(),
        ));
    }
    for h in [
        Halt::Calls,
        Halt::Tokens,
        Halt::Spend,
        Halt::Wall,
        Halt::Broken,
    ] {
        checks.push(says(&page, &format!("Halt::{h:?} label"), h.label()));
        // The sentence is what a viewer reads, so it is pinned verbatim.
        checks.push(says(
            &page,
            &format!("Halt::{h:?} public sentence"),
            h.public_text(),
        ));
    }

    // -- 3. the vocabulary, driven through the real parser ------------------
    for (verb, body) in [
        ("press", r#"{"do":"press","button":"a"}"#),
        ("wait", r#"{"do":"wait","frames":30}"#),
        ("walk", r#"{"do":"walk","dir":"down","steps":2}"#),
        ("goto", r#"{"do":"goto","x":3,"y":7}"#),
        ("advance_text", r#"{"do":"advance_text"}"#),
    ] {
        checks.push(verb_parses(
            &format!("the page advertises `{verb}` and it parses"),
            body,
            true,
        ));
        checks.push(says(&page, &format!("`{verb}` is on the page"), verb));
    }
    for (alias, body) in [
        ("tap", r#"{"do":"tap","button":"a"}"#),
        ("button", r#"{"do":"button","button":"a"}"#),
        ("move", r#"{"do":"move","dir":"up"}"#),
        ("go_to", r#"{"do":"go_to","x":1,"y":1}"#),
        ("walk_to", r#"{"do":"walk_to","x":1,"y":1}"#),
        ("advance", r#"{"do":"advance"}"#),
        ("talk", r#"{"do":"talk"}"#),
        ("text", r#"{"do":"text"}"#),
    ] {
        checks.push(verb_parses(
            &format!("documented alias `{alias}` really parses"),
            body,
            true,
        ));
    }
    // Field aliases the page lists.
    for (what, body) in [
        ("`action` for `do`", r#"{"action":"press","button":"a"}"#),
        ("`key` for `button`", r#"{"do":"press","key":"a"}"#),
        ("`direction` for `dir`", r#"{"do":"walk","direction":"up"}"#),
        (
            "`count` for `times`",
            r#"{"do":"press","button":"a","count":3}"#,
        ),
    ] {
        checks.push(verb_parses(what, body, true));
    }
    checks.push(verb_parses(
        "an unknown verb is rejected, as the page says",
        r#"{"do":"teleport","x":1,"y":1}"#,
        false,
    ));

    // Clamping: the page says numbers are bounded, so prove one.
    let over = json::parse(r#"{"do":"walk","dir":"up","steps":9999}"#)
        .ok()
        .and_then(|v| Action::from_json(&v, &lim).ok());
    checks.push(Check {
        what: "an over-large `steps` is clamped, not refused".into(),
        ok: matches!(over, Some(Action::Walk { steps, .. }) if steps == lim.max_steps),
        detail: format!("{over:?}, max_steps {}", lim.max_steps),
    });

    // -- 4. the schema and the generated examples ---------------------------
    checks.push(says(
        &page,
        "the snapshot schema string",
        terminalgb_plugin_agent::SCHEMA,
    ));

    let ex = Path::new("docs/agent/examples");
    for f in [
        "mcp-transcript.md",
        "mcp-transcript.jsonl",
        "tools-list.json",
        "agent-snapshots.md",
        "agent-run.txt",
        "screen-tour.txt",
    ] {
        let rel = ex.join(f);
        let present = root().join(&rel).exists();
        checks.push(Check {
            what: format!("generated example {f} is committed"),
            ok: present,
            detail: rel.display().to_string(),
        });
        checks.push(Check {
            what: format!("the page links {f}"),
            ok: page.contains(f),
            detail: String::new(),
        });
    }

    // Every tool the server really lists must be named on the page. This is
    // the one claim the page cannot check against a Rust constant — the tool
    // table is private to `src/mcp` and that module is behind another feature
    // — so it is checked against the captured `tools/list` reply instead.
    match read("docs/agent/examples/tools-list.json").and_then(|s| json::parse(&s).ok()) {
        Some(v) => {
            let tools = v
                .get("result")
                .and_then(|r| r.get("tools"))
                .and_then(|t| t.as_array())
                .map(|a| a.to_vec())
                .unwrap_or_default();
            checks.push(Check {
                what: "the captured tools/list is not empty".into(),
                ok: !tools.is_empty(),
                detail: format!("{} tools", tools.len()),
            });
            for t in &tools {
                let name = t.str_field("name").unwrap_or_default();
                checks.push(Check {
                    what: format!("tool `{name}` is documented"),
                    ok: page.contains(&format!("`{name}`")),
                    detail: String::new(),
                });
            }
            // The one tool that must NOT be in a default listing.
            checks.push(Check {
                what: "write_memory is absent from a default tools/list".into(),
                ok: !tools
                    .iter()
                    .any(|t| t.str_field("name") == Some("write_memory")),
                detail: "captured without --allow-write-memory".into(),
            });
        }
        None => checks.push(Check {
            what: "docs/agent/examples/tools-list.json parses".into(),
            ok: false,
            detail: "missing or unreadable".into(),
        }),
    }

    // The snapshots really are the schema the page documents.
    match read("docs/agent/examples/agent-snapshots.md") {
        Some(s) => checks.push(Check {
            what: "the captured snapshots carry the documented schema".into(),
            ok: s.contains(terminalgb_plugin_agent::SCHEMA),
            detail: format!("looked for {:?}", terminalgb_plugin_agent::SCHEMA),
        }),
        None => checks.push(Check {
            what: "agent-snapshots.md is readable".into(),
            ok: false,
            detail: String::new(),
        }),
    }

    // -- 5. docs/agent-environment.md: the two numbers it rests on ----------
    //
    // The determinism claim is a *hash*, and a hash written into prose is the
    // most stale-able thing in this repository. It is recomputed here from the
    // vendored MIT cartridge, the same way `examples/headless.rs` does, so the
    // page cannot quietly keep quoting a number the emulator no longer
    // produces. The size beside it is the reset primitive's, from a real
    // `save_state`.
    match read("docs/agent-environment.md") {
        Some(env_page) => {
            let rom = std::fs::read(root().join("third_party/dmg-acid2/dmg-acid2.gb"));
            match rom {
                Ok(rom) => {
                    let mut gb = terminalgb::gameboy::Gameboy::new(rom, None);
                    for i in 0..300u32 {
                        if i == 150 {
                            gb.keydown(terminalgb::KeypadKey::Start);
                        }
                        if i == 156 {
                            gb.keyup(terminalgb::KeypadKey::Start);
                        }
                        gb.frame();
                    }
                    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                    for b in gb.image() {
                        h ^= *b as u64;
                        h = h.wrapping_mul(0x0000_0100_0000_01b3);
                    }
                    let hash = format!("{h:#018x}");
                    checks.push(Check {
                        what: "the cross-architecture frame hash on the page is current"
                            .into(),
                        ok: env_page.contains(&hash),
                        detail: format!("the emulator produces {hash}"),
                    });
                    let bytes = gb.save_state().len();
                    checks.push(Check {
                        what: "the save-state size on the page is current".into(),
                        ok: env_page.contains(&bytes.to_string()),
                        detail: format!("a real save_state is {bytes} bytes"),
                    });
                }
                Err(e) => checks.push(Check {
                    what: "the vendored dmg-acid2 cartridge is readable".into(),
                    ok: false,
                    detail: e.to_string(),
                }),
            }
            // The page's whole argument rests on saying which claims have been
            // run, so the section may not quietly disappear.
            for needle in [
                "What has been run, and what has not",
                "Nobody has trained a",
                "frames per model call",
                "Scoring that cannot be gamed",
            ] {
                checks.push(says(
                    &env_page,
                    "agent-environment.md keeps its honesty section",
                    needle,
                ));
            }
        }
        None => checks.push(Check {
            what: "docs/agent-environment.md is readable".into(),
            ok: false,
            detail: String::new(),
        }),
    }

    // -- report -------------------------------------------------------------
    println!("\nagent documentation gate\n");
    let mut bad = 0;
    for c in &checks {
        if c.ok {
            println!("  [PASS] {}", c.what);
        } else {
            bad += 1;
            println!("  [FAIL] {}\n         {}", c.what, c.detail);
        }
    }
    println!("\n{} of {} passed", checks.len() - bad, checks.len());
    if bad > 0 {
        println!(
            "\ndocs/agent-reference.md disagrees with the code. Fix the page — or, if the\n\
             examples moved, re-run:\n\
             \x20 python3 tools/agent-docs/capture.py --rom <blue.gb> --save <blue.sav>"
        );
        std::process::exit(1);
    }
}

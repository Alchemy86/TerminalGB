//! **The scenario front end** — write a goal and its rules in a small JSON
//! file, play it, record what happened, replay the recording for nothing.
//!
//! ```text
//! cargo run --release --example scenario --                        # what is here
//! cargo run --release --example scenario -- check    scenarios/
//! cargo run --release --example scenario -- run      --rom <rom> --scenario <s> [--driver …]
//! cargo run --release --example scenario -- replay   --rom <rom> --scenario <s> --recording <r>
//! cargo run --release --example scenario -- knowledge --search badges
//! cargo run --release --example scenario -- guidance  <guidance.json>
//! ```
//!
//! A walkthrough that builds one scenario end to end:
//! [`docs/writing-a-scenario.md`](../docs/writing-a-scenario.md).
//!
//! # Two of the five subcommands cost nothing and need no cartridge
//!
//! `check` validates a scenario and prints its **cost ceiling before anything
//! is spent** — the captain's money, so the number is on screen first rather
//! than discoverable afterwards from a bill. `knowledge` answers a question out
//! of the vendored AtlasGB snapshot. Neither loads a ROM.
//!
//! # `replay` makes no model calls at all
//!
//! A recording is the driver's replies in order; a
//! [`terminalgb_plugin_agent::ScriptedDriver`] answers from that list through
//! the identical parse-clamp-execute path the live run took. So a replay is not
//! an approximation of the run, and it goes at the emulator's own speed rather
//! than the network's. What the harness measures is printed at the end.
//!
//! # The ROM is staged, always
//!
//! The MBC writes a `.sav` beside the cartridge, so a run pointed at somebody's
//! ROM directory would write into it. Everything here copies the cartridge into
//! the output directory first — the standing rule in `AGENTS.md`.
//!
//! # Output is for an agent first
//!
//! An agent drives this by shell and **does not read stderr**, so every answer
//! *and every error* goes to stdout, shaped as TOON: a `name[n]{col,col}:`
//! header and one row per line, or a `name:` block of `key: value`. An error is
//! `error:` plus one or more `help:` lines naming the command that fixes it.
//! Exit codes are 0 success, 1 the work failed, 2 the command line was wrong.
//! `--prose` restores the paragraphs a person may prefer to read.

use std::path::{Path, PathBuf};
use std::process::exit;

use terminalgb::gameboy::Gameboy;
use terminalgb::scenario::knowledge::{Atlas, Guidance};
use terminalgb::scenario::record::Recording;
use terminalgb::scenario::run::ScenarioRun;
use terminalgb::scenario::Scenario;
use terminalgb_plugin_agent::driver::{Driver, RandomDriver, ScriptedDriver};
use terminalgb_plugin_agent::governor::Budget;
use terminalgb_plugin_agent::subprocess::CommandDriver;
use terminalgb_plugin_agent::{AgentPlugin, Config};

/// How the tool names itself in every suggestion it prints, so a suggestion can
/// be pasted rather than translated.
const SELF: &str = "cargo run --release --example scenario --";

const DESCRIPTION: &str = "Author, check, play and replay scenarios — a goal, \
                           rules the host enforces rather than asks for, and a \
                           run recorded as a file that replays with no model \
                           calls";

/// Where `check` and the home view look when nobody names a file.
const SCENARIO_ROOT: &str = "scenarios";

// -- output -------------------------------------------------------------------

/// A TOON cell, quoted only when it would otherwise be ambiguous.
fn cell(v: &str) -> String {
    if v.is_empty()
        || v.trim() != v
        || v.contains(',')
        || v.contains('"')
        || v.contains(':')
        || v.contains('\n')
    {
        format!("\"{}\"", v.replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// One TOON table: `name[n]{cols}:` then one indented row per line.
fn table(name: &str, cols: &[&str], rows: &[Vec<String>]) {
    println!("{name}[{}]{{{}}}:", rows.len(), cols.join(","));
    for r in rows {
        println!(
            "  {}",
            r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",")
        );
    }
}

/// One TOON object: `name:` then `  key: value`.
fn obj(name: &str, pairs: &[(&str, String)]) {
    println!("{name}:");
    for (k, v) in pairs {
        println!("  {k}: {v}");
    }
}

/// Contextual next steps — complete commands, placeholders rather than guesses.
fn helps(lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    println!("help[{}]:", lines.len());
    for l in lines {
        println!("  {l}");
    }
}

/// A structured error **on stdout**, with what to run instead, and exit.
fn die(message: &str, suggestions: &[String], code: i32) -> ! {
    println!("error: {message}");
    for s in suggestions {
        println!("help: {s}");
    }
    exit(code)
}

/// Long free text costs tokens on every call; show enough and say how much was
/// left, rather than omitting it or printing all of it.
fn clip(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        return t.to_string();
    }
    let head: String = t.chars().take(n).collect();
    format!("{head}… ({} chars total)", t.chars().count())
}

fn home_path(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

// -- help ---------------------------------------------------------------------

const HELP_CHECK: &str = "\
check [PATH] [--prose]
    Validate a scenario and print its cost ceiling. Loads no cartridge and
    costs nothing. PATH is a scenario file, or a directory (every *.json
    beneath it, one row each), and defaults to `scenarios`.

    --prose   the paragraphs, instead of TOON

examples:
    scenario -- check scenarios/leave-the-pokemon-center.json
    scenario -- check scenarios/pewter
    scenario -- check";

const HELP_KNOWLEDGE: &str = "\
knowledge [--symbol NAME | --at 0xNNNN | --search TEXT] [--limit N]
    Ask the vendored AtlasGB snapshot which address records something. Loads
    no cartridge and costs nothing. With no query it reports what the atlas is.

    --symbol NAME   one address by its pokered symbol, e.g. wCurMap
    --at 0xNNNN     which entry covers an address, e.g. 0xD35E
    --search TEXT   symbols and descriptions containing TEXT
    --limit N       rows for --search (default 12)
    $GB_ATLAS       an atlas.tsv elsewhere

examples:
    scenario -- knowledge --search badges
    scenario -- knowledge --symbol wEventFlags
    scenario -- knowledge --at 0xD35E";

const HELP_GUIDANCE: &str = "\
guidance FILE
    Print the map graph earlier runs walked — every door a run went through,
    and how many runs found it. A scenario names its own guidance file; a file
    that does not exist yet is the normal beginning of one.

example:
    scenario -- guidance scenarios/pewter/guidance/02-leave-the-house.json";

const HELP_RUN: &str = "\
run --rom FILE --scenario FILE [options]
    Play a scenario. Stages the cartridge into --out first, always.

    --rom FILE          the cartridge                        (required)
    --scenario FILE     the scenario                         (required)
    --start FILE        save state every attempt begins from
                        (default: the scenario's own start_state)
    --driver KIND       scripted | random | command          (default scripted)
    --script FILE       one JSON reply per line, for --driver scripted
    --seed N            for --driver random                  (default 1)
    --command CMD ...   the adapter, for --driver command    (MUST BE LAST:
                        it swallows every remaining argument)
    --out DIR           recording and captures
                        (default target/scenario/<name>)
    --pace-thinking     run at real time while a model call is in flight.
                        Turn this ON for anything with a live model in it.
    --repeat N          play it N times and report the success rate
    --prose             the paragraphs, instead of TOON

examples:
    scenario -- run --rom blue.gb --scenario s.json --driver random --seed 1
    scenario -- run --rom blue.gb --scenario s.json --repeat 20 --driver random
    scenario -- run --rom blue.gb --scenario s.json --pace-thinking \\
        --driver command --command python3 adapter.py";

const HELP_REPLAY: &str = "\
replay --rom FILE --scenario FILE --recording FILE [--start FILE] [--out DIR]
    Replay a recording through the same parse-clamp-execute path the run took.
    Makes NO model calls. Exit 0 when the replay is faithful, 1 when it is not.

example:
    scenario -- replay --rom blue.gb --scenario s.json \\
        --recording target/scenario/s/run-1.json";

fn subcommand_help(name: &str) -> Option<&'static str> {
    Some(match name {
        "check" => HELP_CHECK,
        "knowledge" => HELP_KNOWLEDGE,
        "guidance" => HELP_GUIDANCE,
        "run" => HELP_RUN,
        "replay" => HELP_REPLAY,
        _ => return None,
    })
}

fn wants_help(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "-h" || a == "--help" || a == "help")
}

const COMMANDS: &[(&str, &str)] = &[
    (
        "check",
        "validate a scenario and print its cost ceiling — no cartridge",
    ),
    (
        "knowledge",
        "ask the memory atlas which address records something",
    ),
    ("guidance", "print the map graph earlier runs walked"),
    ("run", "play a scenario and record it"),
    ("replay", "replay a recording — no model calls"),
];

// -- entry --------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        cmd_home();
        return;
    }
    match args[0].as_str() {
        "-h" | "--help" | "help" => {
            // `help <subcommand>` answers about that one, not the whole manual.
            match args.get(1).and_then(|n| subcommand_help(n)) {
                Some(text) => println!("{text}"),
                None => cmd_home(),
            }
        }
        "check" => cmd_check(&args[1..]),
        "knowledge" => cmd_knowledge(&args[1..]),
        "guidance" => cmd_guidance(&args[1..]),
        "run" => cmd_run(&args[1..], false),
        "replay" => cmd_run(&args[1..], true),
        other => {
            let near: Vec<&str> = COMMANDS
                .iter()
                .map(|(n, _)| *n)
                .filter(|n| n.starts_with(&other[..1.min(other.len())]))
                .collect();
            let mut help = vec![format!(
                "commands are: {}",
                COMMANDS
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            )];
            if !near.is_empty() {
                help.push(format!("did you mean: {}", near.join(", ")));
            }
            help.push(format!("Run `{SELF}` to see what is here"));
            die(&format!("{other:?} is not a subcommand"), &help, 2)
        }
    }
}

// -- the home view ------------------------------------------------------------

/// Every `*.json` beneath a directory, two levels deep, sorted.
///
/// `guidance/` is skipped by name: a guidance file is a `gbguidance/1`
/// document a *run* wrote, so listing one as an unreadable scenario would put
/// a wall of red beside a repository where nothing is wrong.
fn find_scenarios(root: &Path) -> Vec<PathBuf> {
    fn push(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> =
            rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if depth > 0 && p.file_name().is_some_and(|n| n != "guidance") {
                    push(&p, out, depth - 1);
                }
            } else if p.extension().is_some_and(|e| e == "json") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    push(root, &mut out, 2);
    out
}

/// A scenario's row in a listing: enough to choose one, not enough to read it.
fn row(path: &Path, s: &Scenario) -> Vec<String> {
    vec![
        path.display().to_string(),
        s.name.clone(),
        s.goal
            .iter()
            .map(|c| c.source.clone())
            .collect::<Vec<_>>()
            .join(" AND "),
        s.rules.len().to_string(),
        if s.evidence.is_empty() {
            "NONE".into()
        } else {
            s.evidence.len().to_string()
        },
    ]
}

fn cmd_home() {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("scenario"));
    obj(
        "bin",
        &[
            ("path", home_path(&exe)),
            ("description", DESCRIPTION.to_string()),
        ],
    );
    match Atlas::load() {
        Some(a) => obj(
            "atlas",
            &[
                ("addresses", a.len().to_string()),
                (
                    "from",
                    a.source()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                ),
            ],
        ),
        None => obj(
            "atlas",
            &[
                ("addresses", "0".into()),
                ("from", format!("MISSING — expected {}", Atlas::VENDORED)),
            ],
        ),
    }
    table(
        "commands",
        &["command", "what it does"],
        &COMMANDS
            .iter()
            .map(|(n, d)| vec![n.to_string(), d.to_string()])
            .collect::<Vec<_>>(),
    );

    let root = PathBuf::from(SCENARIO_ROOT);
    let files = find_scenarios(&root);
    let mut rows = Vec::new();
    let mut broken = Vec::new();
    for f in &files {
        match Scenario::load(f) {
            Ok(s) => rows.push(row(f, &s)),
            Err(e) => broken.push(vec![f.display().to_string(), clip(&e, 90)]),
        }
    }
    if files.is_empty() {
        println!(
            "scenarios: 0 found beneath {SCENARIO_ROOT}/ — that directory holds \
             this repository's own, and a scenario is just a JSON file anywhere"
        );
    } else {
        table(
            "scenarios",
            &["file", "name", "goal", "rules", "evidence"],
            &rows,
        );
    }
    if !broken.is_empty() {
        table("unreadable", &["file", "why"], &broken);
    }

    helps(&[
        format!("Run `{SELF} check <file>` to validate one and see its cost ceiling — no cartridge, no cost"),
        format!("Run `{SELF} check {SCENARIO_ROOT}` to validate every one of them at once"),
        format!("Run `{SELF} knowledge --search <text>` to ask which address records something"),
        format!("Run `{SELF} run --rom <rom.gb> --scenario <file> --driver random --seed 1` to play one"),
        format!("Run `{SELF} help <command>` for a command's own flags and examples"),
        "Read docs/writing-a-scenario.md to build one from an empty file".into(),
    ]);
}

// -- check --------------------------------------------------------------------

fn cmd_check(args: &[String]) {
    if wants_help(args) {
        println!("{HELP_CHECK}");
        return;
    }
    let mut prose = false;
    let mut target: Option<PathBuf> = None;
    for a in args {
        match a.as_str() {
            "--prose" => prose = true,
            other if other.starts_with("--") => die(
                &format!("unknown option {other:?} for check"),
                &[format!("Run `{SELF} help check` for its flags")],
                2,
            ),
            other => target = Some(PathBuf::from(other)),
        }
    }
    let path = target.unwrap_or_else(|| PathBuf::from(SCENARIO_ROOT));

    if path.is_dir() {
        check_directory(&path);
        return;
    }
    if !path.exists() {
        die(
            &format!("no such scenario file {}", path.display()),
            &[
                format!("Run `{SELF}` to list the scenarios that are here"),
                format!("Run `{SELF} check {SCENARIO_ROOT}` to check them all"),
            ],
            1,
        );
    }
    match Scenario::load(&path) {
        Ok(s) => {
            if prose {
                check_prose(&path, &s);
            } else {
                check_toon(&path, &s);
            }
        }
        Err(e) => die(
            &format!("{}: {e}", path.display()),
            &[
                format!("Run `{SELF} help check` for what a scenario must carry"),
                "Read docs/writing-a-scenario.md for the field-by-field walkthrough"
                    .into(),
            ],
            1,
        ),
    }
}

/// A whole tree at once — the shape an agent wants when it has just edited one
/// file and needs to know nothing else broke.
fn check_directory(dir: &Path) {
    let files = find_scenarios(dir);
    if files.is_empty() {
        println!("scenarios: 0 *.json files beneath {}", dir.display());
        helps(&[format!(
            "Run `{SELF} check <file.json>` to check one somewhere else"
        )]);
        return;
    }
    let mut rows = Vec::new();
    let mut bad = 0;
    for f in &files {
        match Scenario::load(f) {
            Ok(s) => rows.push(vec![
                f.display().to_string(),
                s.name.clone(),
                "OK".into(),
                if s.evidence.is_empty() {
                    "NONE STATED".into()
                } else {
                    format!("{} line(s)", s.evidence.len())
                },
            ]),
            Err(e) => {
                bad += 1;
                rows.push(vec![
                    f.display().to_string(),
                    String::new(),
                    "REFUSED".into(),
                    clip(&e, 120),
                ]);
            }
        }
    }
    table("checked", &["file", "name", "verdict", "evidence"], &rows);
    obj(
        "summary",
        &[
            ("checked", files.len().to_string()),
            ("ok", (files.len() - bad).to_string()),
            ("refused", bad.to_string()),
        ],
    );
    if bad > 0 {
        helps(&[format!(
            "Run `{SELF} check <the refused file>` for the whole message"
        )]);
        exit(1);
    }
}

fn check_toon(path: &Path, s: &Scenario) {
    let ep = s.episode();
    obj(
        "scenario",
        &[
            ("name", s.name.clone()),
            ("file", path.display().to_string()),
            ("objective", clip(&s.objective, 400)),
            (
                "goal",
                s.goal
                    .iter()
                    .map(|c| c.source.clone())
                    .collect::<Vec<_>>()
                    .join(" AND "),
            ),
            (
                "failure",
                if s.failure.is_empty() {
                    "none".into()
                } else {
                    s.failure
                        .iter()
                        .map(|c| c.source.clone())
                        .collect::<Vec<_>>()
                        .join(" OR ")
                },
            ),
            (
                "start_state",
                match s.start_state_from(path) {
                    Some(p) if p.exists() => p.display().to_string(),
                    Some(p) => format!("{} (MISSING)", p.display()),
                    None => "none — every attempt boots the cartridge cold".into(),
                },
            ),
            ("timeout_frames", s.timeout_frames.to_string()),
            ("battles", s.referee().battle_policy().name().to_string()),
        ],
    );
    obj(
        "cost",
        &[
            ("attempts", "1".into()),
            ("max_calls_each", ep.max_calls.to_string()),
            ("model_calls_worst_case", ep.max_calls.to_string()),
            ("frames_worst_case", s.timeout_frames.to_string()),
        ],
    );
    table(
        "rules",
        &["rule", "as written"],
        &s.rules
            .iter()
            .map(|r| vec![r.name().to_string(), r.source()])
            .collect::<Vec<_>>(),
    );
    if s.watches.is_empty() {
        println!("watches: 0 declared — the goal uses only the built-in vocabulary");
    } else {
        table(
            "watches",
            &["name", "addr", "len", "mask"],
            &s.watches
                .0
                .iter()
                .map(|w| {
                    vec![
                        w.name.clone(),
                        format!("0x{:04X}", w.addr),
                        w.len.to_string(),
                        if w.mask == 0 {
                            "none — the whole byte".into()
                        } else {
                            format!("0x{:02X}", w.mask)
                        },
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    let k = s.knowledge_from(path);
    let mut krows: Vec<Vec<String>> = k
        .sources
        .iter()
        .map(|src| vec![src.clone(), "available".into()])
        .collect();
    for m in &k.missing {
        krows.push(vec![m.clone(), "MISSING".into()]);
    }
    if krows.is_empty() {
        println!("knowledge: 0 sources — this scenario asks for none");
    } else {
        table("knowledge", &["source", "state"], &krows);
    }
    if s.evidence.is_empty() {
        println!(
            "evidence: NONE STATED — nothing in this file says which address \
             defines success, so a reader cannot check the goal is the game's \
             own record of the milestone rather than somebody's idea of it"
        );
    } else {
        table(
            "evidence",
            &["for", "citation"],
            &s.evidence
                .iter()
                .map(|e| {
                    // `<condition> — <citation>` is the shape these are written
                    // in, but an em dash also turns up mid-sentence, so only a
                    // short head is treated as naming a condition.
                    match e.split_once(" — ") {
                        Some((head, rest)) if head.chars().count() <= 48 => {
                            vec![head.to_string(), clip(rest, 300)]
                        }
                        _ => vec!["-".into(), clip(e, 300)],
                    }
                })
                .collect::<Vec<_>>(),
        );
    }
    // Say what the agent will actually be told, because a rule stated only to
    // the host is a rule the agent is judged on without ever having heard of it.
    let told = s.referee().instructions();
    if told.is_empty() {
        println!("briefing: 0 behavioural rules — the agent is told nothing beyond the objective");
    } else {
        table(
            "briefing",
            &["the agent is told"],
            &told.iter().map(|n| vec![n.clone()]).collect::<Vec<_>>(),
        );
    }
    println!("verdict: OK");
    let mut h = vec![format!(
        "Run `{SELF} run --rom <rom.gb> --scenario {} --driver random --seed 1` to play it",
        path.display()
    )];
    if s.evidence.is_empty() {
        h.push(
            "Add an `evidence` array: one line per goal condition naming the address \
             and where the cartridge defines it"
                .into(),
        );
    }
    if s.start_state_from(path).is_some_and(|p| !p.exists()) {
        h.push(
            "Its start_state does not exist yet — pass `--start <file.gbss>` or make one"
                .to_string(),
        );
    }
    helps(&h);
}

fn check_prose(path: &Path, s: &Scenario) {
    for line in s.briefing(1) {
        println!("{line}");
    }
    let k = s.knowledge_from(path);
    for src in &k.sources {
        println!("  knowledge available: {src}");
    }
    for m in &k.missing {
        println!("  knowledge MISSING:   {m}");
    }
    println!("\nthe agent will be told:");
    for note in s.referee().instructions() {
        println!("  - {note}");
    }
    println!("\nOK");
}

// -- knowledge ----------------------------------------------------------------

fn cmd_knowledge(args: &[String]) {
    if wants_help(args) {
        println!("{HELP_KNOWLEDGE}");
        return;
    }
    let Some(atlas) = Atlas::load() else {
        die(
            &format!(
                "no memory atlas — expected {} beneath the repository root",
                Atlas::VENDORED
            ),
            &[
                "Run `tools/fetch-atlas.sh --verify` to check the vendored copy".into(),
                "or set $GB_ATLAS to an atlas.tsv elsewhere".into(),
            ],
            1,
        );
    };
    obj(
        "atlas",
        &[
            ("addresses", atlas.len().to_string()),
            (
                "from",
                atlas
                    .source()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
        ],
    );

    let cols = ["symbol", "addr", "bytes", "evidence", "what it is"];
    let entry_row = |e: &terminalgb::scenario::knowledge::Entry| {
        vec![
            e.symbol.clone(),
            format!("${:04X}", e.addr),
            e.len.to_string(),
            if e.verify.is_empty() {
                "NONE".into()
            } else {
                e.verify.clone()
            },
            clip(&e.desc, 240),
        ]
    };

    let mut limit = 12usize;
    let mut i = 0;
    let mut asked = false;
    while i < args.len() {
        let value = |i: usize, flag: &str| -> String {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                die(
                    &format!("{flag} needs a value"),
                    &[format!("Run `{SELF} help knowledge` for its flags")],
                    2,
                )
            })
        };
        match args[i].as_str() {
            "--limit" => {
                limit = value(i, "--limit").parse().unwrap_or_else(|_| {
                    die(
                        "--limit needs a whole number",
                        &[format!("Run `{SELF} knowledge --search <text> --limit 25`")],
                        2,
                    )
                });
                i += 2;
            }
            "--symbol" => {
                asked = true;
                let name = value(i, "--symbol");
                match atlas.symbol(&name) {
                    Some(e) => table("entry", &cols, &[entry_row(e)]),
                    None => {
                        let close = atlas.search(&name, 5);
                        println!("entry: 0 — no symbol named {name:?} in the atlas");
                        if !close.is_empty() {
                            table(
                                "near",
                                &cols,
                                &close.into_iter().map(entry_row).collect::<Vec<_>>(),
                            );
                        }
                    }
                }
                i += 2;
            }
            "--at" => {
                asked = true;
                let raw = value(i, "--at");
                let t = raw
                    .trim_start_matches("0x")
                    .trim_start_matches("0X")
                    .trim_start_matches('$');
                let Ok(a) = u16::from_str_radix(t, 16) else {
                    die(
                        &format!("{raw:?} is not a 16-bit address"),
                        &[format!("Run `{SELF} knowledge --at 0xD35E`")],
                        2,
                    )
                };
                match atlas.at(a) {
                    Some(e) => {
                        obj("query", &[("address", format!("${a:04X}"))]);
                        table("covers", &cols, &[entry_row(e)]);
                    }
                    None => println!("covers: 0 — ${a:04X} is in no entry in the atlas"),
                }
                i += 2;
            }
            "--search" => {
                asked = true;
                let needle = value(i, "--search");
                let hits = atlas.search(&needle, limit);
                if hits.is_empty() {
                    println!("matches: 0 — nothing in the atlas mentions {needle:?}");
                    helps(&[format!(
                        "Run `{SELF} knowledge --search <a shorter fragment>`"
                    )]);
                } else {
                    table(
                        "matches",
                        &cols,
                        &hits.into_iter().map(entry_row).collect::<Vec<_>>(),
                    );
                    helps(&[format!(
                        "Run `{SELF} knowledge --symbol <symbol>` for one of them on its own"
                    )]);
                }
                i += 2;
            }
            other => die(
                &format!("unknown option {other:?} for knowledge"),
                &[format!("Run `{SELF} help knowledge` for its flags")],
                2,
            ),
        }
    }
    if !asked {
        helps(&[
            format!("Run `{SELF} knowledge --search <text>` to find an address by what it records"),
            format!("Run `{SELF} knowledge --symbol wCurMap` for one by its pokered symbol"),
            format!("Run `{SELF} knowledge --at 0xD35E` to ask what covers an address"),
        ]);
    }
}

// -- guidance -----------------------------------------------------------------

fn cmd_guidance(args: &[String]) {
    if wants_help(args) {
        println!("{HELP_GUIDANCE}");
        return;
    }
    let Some(file) = args.first() else {
        die(
            "guidance needs a file",
            &[
                format!("Run `{SELF} guidance <guidance.json>`"),
                format!("Run `{SELF} check <scenario.json>` to see which file a scenario uses"),
            ],
            2,
        )
    };
    match Guidance::load(Path::new(file)) {
        Ok(g) => {
            obj(
                "guidance",
                &[("file", file.clone()), ("rom", g.rom_title.clone())],
            );
            if g.links.is_empty() {
                println!(
                    "links: 0 — no door has been walked yet, so nothing has run \
                     this scenario and there is no route to hand the next agent"
                );
            } else {
                table(
                    "links",
                    &["from", "from_id", "door_x", "door_y", "to", "to_id", "uses"],
                    &g.links
                        .iter()
                        .map(|l| {
                            vec![
                                terminalgb::longplay::map_name(l.from).to_string(),
                                format!("0x{:02X}", l.from),
                                l.x.to_string(),
                                l.y.to_string(),
                                terminalgb::longplay::map_name(l.to).to_string(),
                                format!("0x{:02X}", l.to),
                                l.uses.to_string(),
                            ]
                        })
                        .collect::<Vec<_>>(),
                );
            }
            let mut notes: Vec<Vec<String>> = Vec::new();
            for (map, ns) in &g.notes {
                for n in ns {
                    notes.push(vec![
                        terminalgb::longplay::map_name(*map).to_string(),
                        n.clone(),
                    ]);
                }
            }
            for n in &g.general {
                notes.push(vec!["(any map)".into(), n.clone()]);
            }
            if notes.is_empty() {
                println!("notes: 0 — nothing has been written down about a map yet");
            } else {
                table("notes", &["map", "note"], &notes);
            }
        }
        Err(e) => die(
            &format!("{file}: {e}"),
            &[
                "A guidance file is written by a run — a missing one is the normal \
                 beginning of a scenario, not an error to fix"
                    .into(),
                format!("Run `{SELF} run --rom <rom.gb> --scenario <file>` to create it"),
            ],
            1,
        ),
    }
}

// -- run / replay -------------------------------------------------------------

#[derive(Default)]
struct RunArgs {
    rom: Option<PathBuf>,
    scenario: Option<PathBuf>,
    start: Option<PathBuf>,
    recording: Option<PathBuf>,
    out: Option<PathBuf>,
    driver: String,
    script: Option<PathBuf>,
    seed: u64,
    command: Vec<String>,
    pace_thinking: bool,
    repeat: u32,
    prose: bool,
}

fn parse_run_args(args: &[String], replay: bool) -> RunArgs {
    let help = if replay { "replay" } else { "run" };
    let mut a = RunArgs {
        driver: "scripted".into(),
        seed: 1,
        repeat: 1,
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        let next = |i: usize| -> String {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                die(
                    &format!("{} needs a value", args[i]),
                    &[format!("Run `{SELF} help {help}` for its flags")],
                    2,
                )
            })
        };
        match args[i].as_str() {
            "--rom" => {
                a.rom = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--scenario" => {
                a.scenario = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--start" => {
                a.start = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--recording" => {
                a.recording = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--out" => {
                a.out = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--driver" => {
                a.driver = next(i);
                i += 2;
            }
            "--script" => {
                a.script = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--seed" => {
                a.seed = next(i).parse().unwrap_or_else(|_| {
                    die(
                        "--seed needs a whole number",
                        &[format!("Run `{SELF} run … --driver random --seed 1`")],
                        2,
                    )
                });
                i += 2;
            }
            "--repeat" => {
                a.repeat = next(i)
                    .parse::<u32>()
                    .unwrap_or_else(|_| {
                        die(
                            "--repeat needs a whole number",
                            &[format!("Run `{SELF} run … --repeat 20`")],
                            2,
                        )
                    })
                    .max(1);
                i += 2;
            }
            "--pace-thinking" => {
                a.pace_thinking = true;
                i += 1;
            }
            "--prose" => {
                a.prose = true;
                i += 1;
            }
            // Swallows the rest, like `examples/claude_plays.rs` — so it must
            // come last, and saying so in the usage is the whole defence.
            "--command" => {
                a.command = args[i + 1..].to_vec();
                break;
            }
            other => die(
                &format!("unknown option {other:?} for {help}"),
                &[format!(
                    "Run `{SELF} help {help}` for its flags and examples"
                )],
                2,
            ),
        }
    }
    a
}

/// Stage the cartridge into the output directory. **Never run against a ROM
/// where it lives** — the MBC writes a `.sav` beside it.
fn stage(rom: &Path, out: &Path) -> PathBuf {
    std::fs::create_dir_all(out).expect("create output directory");
    let dest = out.join(rom.file_name().unwrap_or_else(|| "rom.gb".as_ref()));
    let bytes = std::fs::read(rom).unwrap_or_else(|e| {
        die(
            &format!("cannot read the cartridge {}: {e}", rom.display()),
            &["check the path — this tool ships no game data".into()],
            1,
        )
    });
    std::fs::write(&dest, &bytes).expect("stage the cartridge");
    dest
}

fn build_driver(a: &RunArgs, replay: Option<Vec<String>>) -> Box<dyn Driver> {
    if let Some(replies) = replay {
        return Box::new(ScriptedDriver::new(replies));
    }
    match a.driver.as_str() {
        "scripted" => {
            let replies = match &a.script {
                Some(p) => std::fs::read_to_string(p)
                    .unwrap_or_else(|e| {
                        die(
                            &format!("cannot read the script {}: {e}", p.display()),
                            &["a script is one JSON reply per line".into()],
                            1,
                        )
                    })
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
                None => die(
                    "--driver scripted needs --script FILE",
                    &[
                        format!("Run `{SELF} run … --driver random --seed 1` for a driver that needs no file"),
                        "a script is one JSON reply per line, the same shape a model returns".into(),
                    ],
                    2,
                ),
            };
            Box::new(ScriptedDriver::new(replies))
        }
        "random" => Box::new(RandomDriver::new(a.seed)),
        "command" => {
            if a.command.is_empty() {
                die(
                    "--driver command needs --command CMD ...",
                    &["--command must come LAST: it swallows every remaining argument".into()],
                    2,
                );
            }
            let (prog, rest) = a.command.split_first().unwrap();
            Box::new(CommandDriver::new(prog, rest.to_vec()))
        }
        other => die(
            &format!("unknown driver {other:?}"),
            &[
                "drivers are: scripted (--script FILE), random (--seed N), command (--command CMD …)"
                    .into(),
            ],
            2,
        ),
    }
}

fn cmd_run(args: &[String], replay: bool) {
    if wants_help(args) {
        println!("{}", if replay { HELP_REPLAY } else { HELP_RUN });
        return;
    }
    let what = if replay { "replay" } else { "run" };
    let a = parse_run_args(args, replay);
    let (Some(rom), Some(scenario_path)) = (a.rom.clone(), a.scenario.clone()) else {
        die(
            &format!("{what} needs both --rom and --scenario"),
            &[
                format!("Run `{SELF} help {what}` for its flags and examples"),
                format!("Run `{SELF}` to list the scenarios that are here"),
            ],
            2,
        )
    };
    let s = Scenario::load(&scenario_path).unwrap_or_else(|e| {
        die(
            &format!("{}: {e}", scenario_path.display()),
            &[format!(
                "Run `{SELF} check {}` on its own",
                scenario_path.display()
            )],
            1,
        )
    });
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/scenario").join(&s.name));

    let recorded = if replay {
        let Some(p) = &a.recording else {
            die(
                "replay needs --recording FILE",
                &[
                    format!("Run `{SELF} help replay`"),
                    "a run writes its recording to <out>/run-<n>.json".into(),
                ],
                2,
            )
        };
        Some(Recording::load(p).unwrap_or_else(|e| {
            die(
                &format!("{}: {e}", p.display()),
                &["a recording is written by `run` — check the path".into()],
                1,
            )
        }))
    } else {
        None
    };

    // The start state: the scenario's own, or one named on the command line.
    let start_path = a
        .start
        .clone()
        .or_else(|| s.start_state_from(&scenario_path));
    let start = start_path.as_ref().map(|p| {
        std::fs::read(p).unwrap_or_else(|e| {
            die(
                &format!("cannot read the start state {}: {e}", p.display()),
                &[
                    "a start state is a .gbss save state — pass a different one with --start".into(),
                    format!("Run `{SELF} check {}` to see which one it wants", scenario_path.display()),
                ],
                1,
            )
        })
    });

    // Everything with a cost, before anything is spent.
    if a.prose {
        for line in s.briefing(a.repeat) {
            println!("{line}");
        }
    } else {
        let ep = s.episode();
        obj(
            "plan",
            &[
                ("scenario", s.name.clone()),
                (
                    "goal",
                    s.goal
                        .iter()
                        .map(|c| c.source.clone())
                        .collect::<Vec<_>>()
                        .join(" AND "),
                ),
                ("attempts", a.repeat.to_string()),
                (
                    "model_calls_worst_case",
                    (ep.max_calls as u64 * a.repeat as u64).to_string(),
                ),
                (
                    "frames_worst_case",
                    (s.timeout_frames * a.repeat as u64).to_string(),
                ),
                (
                    "driver",
                    if replay {
                        "recorded".into()
                    } else {
                        a.driver.clone()
                    },
                ),
                (
                    "start_state",
                    start_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "none — the cartridge boots cold".into()),
                ),
                ("out", out.display().to_string()),
            ],
        );
    }
    if let Some(r) = &recorded {
        println!(
            "recording: {} decisions recorded on {:?} — 0 model calls",
            r.decisions.len(),
            r.rom_title
        );
    }
    println!();

    let mut goals = 0u32;
    let mut last_recording = None;
    for attempt in 1..=a.repeat {
        let staged = stage(&rom, &out);
        let data = std::fs::read(&staged).expect("read staged cartridge");
        let mut gb = Gameboy::new(data, Some(staged));
        let driver = build_driver(&a, recorded.as_ref().map(|r| r.replies()));
        let cfg = Config {
            budget: Budget {
                max_calls: s.episode().max_calls,
                ..Budget::default()
            },
            objective: s.objective.clone(),
            log: Some(out.join(format!("agent-{attempt}.ndjson"))),
            ..Config::default()
        };
        gb.add_plugin(Box::new(AgentPlugin::new(driver, cfg)))
            .expect("attach the agent");

        let run =
            ScenarioRun::new(gb, s.clone(), &scenario_path, out.clone(), start.clone())
                .unwrap_or_else(|e| {
                    die(
                        &e,
                        &[format!("Run `{SELF} check {}`", scenario_path.display())],
                        1,
                    )
                })
                .pacing_while_thinking(a.pace_thinking);

        if let Some(r) = &recorded {
            match run.replay(r) {
                Ok(report) => {
                    if a.prose {
                        for line in report.lines() {
                            println!("  {line}");
                        }
                    } else {
                        obj(
                            "replay",
                            &[
                                (
                                    "final_state",
                                    format!(
                                        "{} ({:016x} recorded, {:016x} replayed)",
                                        if report.state_matched {
                                            "MATCHES"
                                        } else {
                                            "DIFFERS"
                                        },
                                        report.recorded_state,
                                        report.replayed_state
                                    ),
                                ),
                                ("frames_recorded", report.recorded_frames.to_string()),
                                ("frames_replayed", report.replayed_frames.to_string()),
                                ("drift", report.drift().to_string()),
                                (
                                    "ending_recorded",
                                    format!("{:?}", report.recorded_ending),
                                ),
                                (
                                    "ending_replayed",
                                    format!("{:?}", report.replayed_ending),
                                ),
                                (
                                    "decisions",
                                    format!(
                                        "{} of {} used",
                                        report.decisions_used, report.decisions_recorded
                                    ),
                                ),
                                ("model_calls", "0".into()),
                                ("wall_ms", report.wall_ms.to_string()),
                                (
                                    "frames_per_second",
                                    format!("{:.0}", report.frames_per_second()),
                                ),
                                ("scenario_drifted", report.scenario_drifted.to_string()),
                            ],
                        );
                    }
                    let path = out.join("replay.json");
                    let _ = std::fs::write(&path, report.to_json());
                    println!("written: {}", path.display());
                    println!(
                        "verdict: {}",
                        if report.faithful() {
                            "FAITHFUL — the recording reproduced exactly"
                        } else {
                            "MISMATCH — the recording did NOT reproduce"
                        }
                    );
                    exit(if report.faithful() { 0 } else { 1 });
                }
                Err(e) => die(
                    &e,
                    &["a recording is refused into a world it was not made in — \
                       check the cartridge and the start state"
                        .into()],
                    1,
                ),
            }
        }

        let result = run.play();
        if a.repeat > 1 {
            println!("attempt: {attempt} of {}", a.repeat);
        }
        for line in result.lines() {
            println!("  {line}");
        }
        if result.reached_goal() {
            goals += 1;
        }
        last_recording = Some(result.recording.clone());
        let path = out.join(format!("run-{attempt}.json"));
        if let Err(e) = result.recording.save(&path) {
            println!("  warning: could not write the recording: {e}");
        } else {
            println!("  recorded: {}", path.display());
        }
        println!();
    }

    if a.repeat > 1 {
        obj(
            "summary",
            &[
                ("attempts", a.repeat.to_string()),
                ("reached_goal", goals.to_string()),
                (
                    "success_rate",
                    format!("{:.0}%", goals as f64 * 100.0 / a.repeat as f64),
                ),
            ],
        );
    }
    // Point at the replay, because a recording nobody replays is a log file.
    if last_recording.is_some() {
        helps(&[format!(
            "Run `{SELF} replay --rom {} --scenario {} --recording {}` to reproduce it for nothing",
            rom.display(),
            scenario_path.display(),
            out.join(format!("run-{}.json", a.repeat)).display()
        )]);
    }
}

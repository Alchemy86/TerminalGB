//! Baseline gating shared by the three external-suite scouts
//! (`mooneye_scout`, `samesuite`, `mealybug`).
//!
//! The suites themselves are **not vendored** — they are somebody else's ROMs
//! under somebody else's licence, fetched at run time exactly as
//! `.github/workflows/conformance-self.yml` fetches Blargg's. What *is* checked
//! in is the expected verdict of every ROM, so a regression in any of the 266
//! external rows is a red build rather than something a human notices weeks
//! later by re-running a suite by hand.
//!
//! The rules are `testharness/baseline.txt`'s rules, deliberately:
//!
//! * **Every expectation is the ROM's own judgement**, never a hash of our
//!   output — Mooneye's Fibonacci register check, SameSuite's `RESULT_CODE`,
//!   Mealybug's author-photographed reference frame.
//! * **Any change is red, in either direction.** A test that starts passing is
//!   as much a baseline change as one that stops.
//! * **Re-blessing is deliberate and reviewable**: `UPDATE_EXTERNAL_BASELINE=1`
//!   rewrites the file, and the diff is what the reviewer reads.
//!
//! A missing baseline file is *not* a silent pass — it is reported as every row
//! being new, and the gate fails.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One ROM's expected result. `value` is the ROM's own report, on one line:
/// `PASS`, or a verdict plus whatever the ROM said about it.
pub struct Row {
    pub key: String,
    pub value: String,
}

impl Row {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Row {
        Row {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Parse a `--baseline <path>` argument out of an example's argv, returning the
/// path and the remaining arguments. Keeps the flag spelling identical across
/// the three scouts.
pub fn take_baseline_arg(args: Vec<String>) -> (Option<PathBuf>, Vec<String>) {
    let mut rest = Vec::new();
    let mut baseline = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--baseline" {
            baseline = it.next().map(PathBuf::from);
        } else {
            rest.push(a);
        }
    }
    (baseline, rest)
}

/// The first line of every baseline file: **which picture mode scored these
/// rows**, and therefore which `make` target re-blesses them.
///
/// There are two renderers and the same suite scores differently on each, so a
/// baseline that does not say which one produced it is a file nobody can
/// safely re-bless — the obvious command would silently rewrite it with the
/// other engine's numbers. See `docs/mealybug.md` §7.
///
/// Both spellings are printed. The modes are named `standard`/`identical`; the
/// `make` targets and every script written before the rename say `fast` and
/// `accurate`, which still parse — a reader who meets one has to be able to
/// join it up with the other.
fn engine_note(path: &Path) -> String {
    let mode = terminalgb::ppu::RenderMode::from_env();
    // A file with no `_accurate_` in its name that is being written by the
    // accurate target is one of the three suites the picture mode does not
    // reach — SameSuite, Mooneye `misc`, the CasualPokePlayer rows. Since
    // `identical` became the default those are gated in *both* modes against
    // this one file, which is only safe because they were measured row-for-row
    // identical under each. Say so, or the header claims a mode that is only
    // half the story.
    let both = mode == terminalgb::ppu::RenderMode::Accurate
        && !path.to_string_lossy().contains("_accurate_");
    if both {
        return "# SCORED IN BOTH PICTURE MODES — this suite measures nothing the\n# picture mode reaches (verified: every row is identical under `standard`\n# and `identical`), so it has one baseline and both gates read it.\n# Re-bless with: UPDATE_EXTERNAL_BASELINE=1 make test-external\n#\n"
            .to_string();
    }
    let target = match mode {
        terminalgb::ppu::RenderMode::Accurate => "make test-external-accurate",
        terminalgb::ppu::RenderMode::Fast => "make test-external",
    };
    format!(
        "# SCORED WITH THE `{name}` PICTURE MODE (GB_PPU={name}, alias `{alias}`).\n# Re-bless with: UPDATE_EXTERNAL_BASELINE=1 {target}\n# The other mode produces different numbers for the same ROMs — see\n# docs/mealybug.md §7 — so re-blessing with the wrong target rewrites this\n# file with the wrong machine's results.\n#\n",
        name = mode.name(),
        alias = mode.alias(),
    )
}

fn load(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// Compare `rows` against the baseline at `path` (or rewrite it when
/// `UPDATE_EXTERNAL_BASELINE` is set). Returns `true` when reality matches.
///
/// `header` is the comment block written above the rows — it is where the
/// suite records *where its ROMs come from*, which is the one thing a reader of
/// a checked-in expectation file cannot otherwise recover.
pub fn gate(suite: &str, path: &Path, header: &str, rows: &[Row]) -> bool {
    if std::env::var("UPDATE_EXTERNAL_BASELINE").is_ok() {
        let mut s = engine_note(path);
        s.push_str(header);
        for r in rows {
            s.push_str(&format!("{} = {}\n", r.key, r.value));
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, s)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        println!(
            "\n{suite}: baseline written to {} ({} rows)",
            path.display(),
            rows.len()
        );
        return true;
    }

    let expected = load(path);
    if expected.is_empty() && !path.exists() {
        eprintln!(
            "\n{suite}: no baseline at {} — nothing is being gated.",
            path.display()
        );
    }

    let mut changed = Vec::new();
    let mut seen = BTreeMap::new();
    for r in rows {
        seen.insert(r.key.clone(), ());
        match expected.get(&r.key) {
            Some(v) if *v == r.value => {}
            Some(v) => changed.push(format!(
                "{}:\n        expected: {v}\n        got:      {}",
                r.key, r.value
            )),
            None => changed.push(format!(
                "{}:\n        expected: <not in baseline>\n        got:      {}",
                r.key, r.value
            )),
        }
    }
    // A ROM that vanished is as much a change as one that broke: it usually
    // means the fetched suite is not the pinned one.
    for k in expected.keys() {
        if !seen.contains_key(k) {
            changed.push(format!(
                "{k}:\n        expected: {}\n        got:      <ROM not present in this run>",
                expected[k]
            ));
        }
    }

    if changed.is_empty() {
        println!(
            "\n{suite}: all {} rows match {}. ✅",
            rows.len(),
            path.display()
        );
        return true;
    }
    println!(
        "\n❌ {suite}: {} row(s) differ from {}:",
        changed.len(),
        path.display()
    );
    for c in &changed {
        println!("    {c}");
    }
    eprintln!(
        "\nFAIL: {suite} baseline mismatch ({} row(s)). Read each diff above; a row that \
         started PASSING is a change too. If it is intended, re-bless with \
         UPDATE_EXTERNAL_BASELINE=1 and commit the diff.",
        changed.len()
    );
    false
}

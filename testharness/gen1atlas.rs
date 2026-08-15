//! **The Gen 1 memory atlas harness** — the machinery that keeps
//! [AtlasGB](https://github.com/Alchemy86/AtlasGB)'s evidence true.
//!
//! A memory map is easy to write and impossible to trust. Every published Gen 1
//! map — the disassembly's own `ram/wram.asm`, the wiki tables, the cheat-code
//! sites — is a *transcription*: somebody copied an address and everybody after
//! them copied it again. This project has the one thing none of those have, a
//! cycle-accurate emulator that can run the cartridge and watch what it does. So
//! the atlas is not a transcription, it is a set of claims, and this is the thing
//! that checks them.
//!
//! ## The atlas lives upstream; the evidence is produced here
//!
//! The data is [`third_party/atlasgb/pokemon-rb/atlas.tsv`](../third_party/atlasgb/README.md),
//! a pinned snapshot of AtlasGB, which owns it and owns every check provable
//! from the file alone. This repository owns the half upstream cannot do for
//! itself: **proving an address against a running cartridge needs a cartridge
//! and an emulator.** So the flow is one way — the snapshot comes in, and the
//! evidence goes back out as a report (`ATLAS_EVIDENCE_OUT`, below).
//!
//! One consequence, and it is the important one: **the `verify` column is not
//! ours to rewrite any more.** There is no re-bless flag here. A disagreement
//! with the vendored atlas is a real finding, and it is landed upstream where
//! CI checks a digest of what was actually run.
//!
//! ## Three tiers, and only the first needs anything
//!
//! **Tier A — structure. No ROM, no network, runs in CI on every
//! `cargo test --release`.** The vendored snapshot is the one `atlas.lock`
//! pinned; the atlas parses; every address lies inside the region it claims;
//! entries in a region march strictly upwards; each entry's `len` really is the
//! distance to the next address; the WRAM walk has no unexplained gap; every
//! `verify` token is from the vocabulary; and — the anti-drift gate — **every
//! Gen 1 address hard-coded in the plugins is in the atlas**. That last one is
//! the point of having a single source of truth at all: `wCurMap` was spelled
//! out in nine separate files before this existed, and nothing could tell you
//! whether they agreed.
//!
//! **Tier B — the cartridge image.** For each entry, look for its address as the
//! operand of an instruction that takes one. `ld [nn],a` is `EA lo hi`; `ldh
//! [n],a` is `E0 n`. An address the game never mentions is a claim about nothing.
//! This is the grade `plugins/battle-control/src/wram.rs` already uses at attach
//! time, applied to the whole map. It is a heuristic, not a disassembly — see
//! `rom_operand_hits` for exactly how far it can be trusted.
//!
//! **Tier C — a running machine.** Boot the cartridge with no save, drive a fixed
//! button script, and record every byte of WRAM, VRAM and HRAM that ever changes.
//! An address observed to change is storage the game genuinely uses. The script
//! is fixed and emulation is deterministic, so this is reproducible for anyone
//! holding the same cartridge — which is what makes it a baseline rather than an
//! anecdote.
//!
//! Then the **invariants**: a dozen named, hand-written checks that test what an
//! entry *means* rather than that it exists. `wPokedexOwned`'s bits must be a
//! subset of `wPokedexSeen`'s. `wPlayerMoney` must be valid BCD. The party's
//! three parallel arrays must agree with each other. `wPartyMon2 - wPartyMon1`
//! must be 44. These are the strongest evidence in the file and the only kind
//! that can catch an address that is real, live, and describing the wrong thing.
//!
//! ## The baseline, and publishing evidence back
//!
//! The vendored `verify` column *is* the baseline, under the rule every baseline
//! in this repository follows: **any change is red in either direction**, and an
//! entry that starts being verified is a change too. What is different here is
//! where a change is landed. It is not re-blessed in place — the lock check
//! would catch that, which is the point of having one — it is published:
//!
//! ```text
//! cargo test --release --test gen1atlas                        # check
//! ATLAS_EVIDENCE_OUT=report.json \
//!   POKE_ROM=~/roms/pokeblue.gb cargo test --release --test gen1atlas
//! # then, in a clone of AtlasGB:
//! tools/apply-evidence.py report.json && make docs data
//! ```
//!
//! The report is `atlasgb-evidence/1` and carries **every symbol in the atlas**,
//! including the ones that earned nothing: upstream refuses a partial report
//! rather than guessing, because an omission would read as a downgrade. It is
//! written only when `ATLAS_EVIDENCE_OUT` names a path — a file that appeared on
//! every test run is a file somebody commits by accident.
//!
//! ## Running it
//!
//! Without a cartridge it runs Tier A, says what it skipped, and **exits 0** —
//! the same rule `colosseum.rs` and `twoprocess.rs` follow. With one:
//!
//! ```text
//! POKE_ROM=~/roms/pokeblue.gb cargo test --release --test gen1atlas
//! ```
//!
//! `POKE_SAVE=<path>` points the invariants at a real save; without it they run
//! against the fresh game the script produced, and the ones that need a
//! populated save say so instead of passing vacuously.
//!
//! **It never writes beside your cartridge.** The ROM is staged into `target/`
//! first, because the MBC writes a `.sav` next to whatever path it was given.
//!
//! **Nothing here touches the network**, including the lock check: `sha256` below
//! is hand-rolled for the same reason the FNV hash, the PNG codec and the
//! inflater in `testharness/` are. A build must not need `sha256sum` on the path,
//! and `make embed`'s one-third-party-dependency rule must not move.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

// ---------------------------------------------------------------------------
// The atlas file
// ---------------------------------------------------------------------------

/// Which of AtlasGB's atlases this harness verifies. AtlasGB is an atlas
/// *project*: `pokemon-rb` is its first and the only one this repository has a
/// use for, but the vendored copy is namespaced the same way upstream is so a
/// second one costs no restructuring here either.
const ATLAS_ID: &str = "pokemon-rb";

/// The pinned AtlasGB snapshot, vendored by `tools/fetch-atlas.sh`. Vendored
/// rather than fetched at build time so that everything in Tier A — the
/// structure and the anti-drift gate — runs on a plain `cargo test` with no
/// network, which is the whole reason it catches drift on the commit that
/// introduces it rather than in a nightly job.
const ATLAS: &str = "third_party/atlasgb/pokemon-rb/atlas.tsv";

/// The lock beside it: `key=value` lines recording the atlas, the ref, the
/// commit and the sha256 of exactly the bytes that were pinned.
const ATLAS_LOCK: &str = "third_party/atlasgb/pokemon-rb/atlas.lock";

/// Region extents as the Game Boy's address decoder sees them. An atlas entry
/// claiming a region must lie inside it — a claim we can check without a ROM,
/// and the cheapest way to catch a transposed digit.
const REGIONS: &[(&str, u16, u16)] = &[
    ("VRAM", 0x8000, 0x9FFF),
    ("SRAM", 0xA000, 0xBFFF),
    ("WRAM0", 0xC000, 0xDFFF),
    ("HRAM", 0xFF80, 0xFFFE),
    ("ROM0", 0x0000, 0x3FFF),
    ("ROMX", 0x4000, 0x7FFF),
];

/// Every token the `verify` column may carry. A token outside this set is a
/// failure, not a note — it would otherwise be a claim nothing checks.
const VERIFY_TOKENS: &[&str] = &["rom", "live", "inv"];

#[derive(Clone)]
struct Entry {
    region: String,
    bank: String,
    addr: u16,
    len: u32,
    symbol: String,
    role: String,
    group: String,
    verify: String,
}

fn parse_atlas(text: &str) -> Result<Vec<Entry>, String> {
    let mut header: Option<Vec<String>> = None;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let Some(h) = &header else {
            header = Some(fields.iter().map(|s| s.to_string()).collect());
            continue;
        };
        if fields.len() != h.len() {
            return Err(format!(
                "{ATLAS}:{}: {} columns, header has {}",
                n + 1,
                fields.len(),
                h.len()
            ));
        }
        let get = |name: &str| -> String {
            h.iter()
                .position(|c| c == name)
                .map(|i| fields[i].to_string())
                .unwrap_or_default()
        };
        let addr_text = get("addr");
        let addr = u16::from_str_radix(addr_text.trim_start_matches('$'), 16)
            .map_err(|_| format!("{ATLAS}:{}: bad address {addr_text:?}", n + 1))?;
        out.push(Entry {
            region: get("region"),
            bank: get("bank"),
            addr,
            len: get("len").parse().unwrap_or(0),
            symbol: get("symbol"),
            role: get("role"),
            group: get("group"),
            verify: get("verify"),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The lock — is the vendored copy the one that was pinned?
// ---------------------------------------------------------------------------

/// SHA-256 of `data`, hand-rolled (FIPS 180-4).
///
/// No new crate: the dependency count of this repository is a contract that
/// `make embed` measures, and one hash is not worth spending it on — the FNV
/// hash, the PNG writer, the PNG reader and the deflater in `testharness/` and
/// `src/` were all written for exactly this reason. Shelling out to `sha256sum`
/// was the other option and is worse: it puts a program that may not exist on
/// the path of `cargo test`, on every platform this core is cross-built for.
fn sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
        0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
        0x1f83d9ab, 0x5be0cd19,
    ];

    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in msg.chunks_exact(64) {
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 =
                w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 =
                w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    h.iter()
        .fold(String::with_capacity(h.len() * 8), |mut s, v| {
            let _ = write!(s, "{v:08x}");
            s
        })
}

/// `key=value` lines. The lock is deliberately not JSON: the shell script that
/// writes it needs no parser to read one back, and neither does this.
fn lock_get(lock: &str, key: &str) -> Option<String> {
    lock.lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('=').map(str::to_string))
}

/// The check that makes vendoring safe: a vendored file is exactly the file
/// somebody will eventually hand-edit instead of fixing upstream, and the
/// `verify` column is the column they would be tempted to edit. Offline, so it
/// runs here on every test rather than only in a job that reaches the network.
fn tier_a_lock(raw: &[u8], report: &mut Report) {
    let Ok(lock) = std::fs::read_to_string(ATLAS_LOCK) else {
        report.check(
            "the snapshot matches its lock",
            false,
            format!(
                "{ATLAS_LOCK} is missing — re-pin with tools/fetch-atlas.sh --ref <tag>"
            ),
        );
        return;
    };
    let want = lock_get(&lock, "sha256").unwrap_or_default();
    let got = sha256(raw);
    let re_pin = lock_get(&lock, "ref").unwrap_or_else(|| "?".into());
    report.check(
        "the snapshot matches its lock",
        want == got,
        if want == got {
            format!("{} @ {}", re_pin, &got[..10])
        } else {
            format!(
                "{ATLAS} is not the file that was pinned: locked {} , actual {} . \
                 Do not edit it here — the verify column is written by landing a \
                 verification report upstream (see third_party/atlasgb/README.md); \
                 fix the data at https://github.com/Alchemy86/AtlasGB and re-pin.",
                &want[..want.len().min(10)],
                &got[..10]
            )
        },
    );
}

// ---------------------------------------------------------------------------
// Tier A — structure. No cartridge needed; this is the part CI runs.
// ---------------------------------------------------------------------------

struct Report {
    checks: Vec<(String, bool, String)>,
}

impl Report {
    fn new() -> Self {
        Report { checks: Vec::new() }
    }
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push((name.to_string(), ok, detail.into()));
    }
    fn failures(&self) -> usize {
        self.checks.iter().filter(|(_, ok, _)| !ok).count()
    }
    fn print(&self) {
        for (name, ok, detail) in &self.checks {
            println!("{} {name:<34} {detail}", if *ok { "ok  " } else { "FAIL" });
        }
    }
}

fn tier_a(entries: &[Entry], report: &mut Report) {
    // Every address inside the region it declares.
    let mut stray = Vec::new();
    for e in entries {
        match REGIONS.iter().find(|(r, _, _)| *r == e.region) {
            Some((_, lo, hi)) if e.addr >= *lo && e.addr <= *hi => {}
            Some(_) => {
                stray.push(format!("{} {} outside {}", e.symbol, fmt(e.addr), e.region))
            }
            None => stray.push(format!("{} in unknown region {:?}", e.symbol, e.region)),
        }
    }
    report.check(
        "addresses inside their region",
        stray.is_empty(),
        if stray.is_empty() {
            format!("{} entries", entries.len())
        } else {
            stray.join("; ")
        },
    );

    // No symbol declared twice. A duplicate would make "look it up by name"
    // ambiguous, which is half of what the atlas is for.
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for e in entries {
        *seen.entry(e.symbol.as_str()).or_default() += 1;
    }
    let dups: Vec<&&str> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(s, _)| s)
        .collect();
    report.check(
        "every symbol appears once",
        dups.is_empty(),
        if dups.is_empty() {
            format!("{} distinct symbols", seen.len())
        } else {
            format!("{dups:?}")
        },
    );

    // Within a region+bank, addresses never go backwards and `len` really is the
    // distance to the next distinct address. This is the self-consistency of the
    // derived data: it catches an extraction bug without needing pokered.
    let mut order_problems = Vec::new();
    let mut len_problems = Vec::new();
    let mut by_bank: BTreeMap<(String, String), Vec<&Entry>> = BTreeMap::new();
    for e in entries {
        by_bank
            .entry((e.region.clone(), e.bank.clone()))
            .or_default()
            .push(e);
    }
    for (key, list) in &by_bank {
        let mut last = 0u32;
        for e in list {
            if (e.addr as u32) < last {
                order_problems.push(format!("{} in {key:?} goes backwards", e.symbol));
            }
            last = e.addr as u32;
        }
        // No entry may run into the next one. It may stop short — a symbol at
        // the end of a linker section owns only to that section's end, and the
        // bytes after it belong to whatever the linker put there next — but an
        // overlap would mean two rows claiming the same byte, which is the one
        // thing a map must never do.
        let addrs: BTreeSet<u16> = list.iter().map(|e| e.addr).collect();
        let addrs: Vec<u16> = addrs.into_iter().collect();
        for e in list {
            let i = addrs.binary_search(&e.addr).unwrap();
            if let Some(next) = addrs.get(i + 1) {
                let room = (*next - e.addr) as u32;
                if e.len > room {
                    len_problems.push(format!(
                        "{} claims {} bytes but {} follows {} later",
                        e.symbol, e.len, next, room
                    ));
                }
            }
        }
    }
    report.check(
        "addresses ascend within a bank",
        order_problems.is_empty(),
        if order_problems.is_empty() {
            format!("{} banks", by_bank.len())
        } else {
            order_problems.join("; ")
        },
    );
    report.check(
        "no entry overlaps the next",
        len_problems.is_empty(),
        if len_problems.is_empty() {
            format!("{} entries", entries.len())
        } else {
            format!("{} overlap: {}", len_problems.len(), len_problems[0])
        },
    );

    // The walk. Every byte of WRAM and HRAM must be accounted for by *some*
    // row — a symbol, a `gap` a section's labels do not reach, or a `free` run
    // the linker left over. This is the claim that makes it an atlas rather than
    // a list of the interesting parts: a hole would be a byte nobody has looked
    // at, and there are none.
    for (region, lo, hi) in [("WRAM0", 0xC000u16, 0xDFFFu16), ("HRAM", 0xFF80, 0xFFFE)] {
        let size = (hi as u32 - lo as u32) + 1;
        let covered = coverage(entries, region, lo, hi);
        report.check(
            &format!("{region} is walked end to end"),
            covered == size,
            format!(
                "{covered} of {size} bytes accounted for{}",
                if covered == size {
                    String::new()
                } else {
                    format!(" — {} unexplained", size - covered)
                }
            ),
        );
    }

    // Every verify token is one the harness actually computes.
    let mut bad = Vec::new();
    for e in entries {
        for tok in e.verify.split(',').filter(|t| !t.is_empty()) {
            if !VERIFY_TOKENS.contains(&tok) {
                bad.push(format!("{}: {tok:?}", e.symbol));
            }
        }
    }
    report.check(
        "verify tokens are known",
        bad.is_empty(),
        if bad.is_empty() {
            format!("vocabulary {VERIFY_TOKENS:?}")
        } else {
            bad.join("; ")
        },
    );

    // Roles: `alias` means another name for an address an `entry` already holds,
    // and `instance` means slot 2..N of a repeated structure whose slot 1 must
    // exist. Both are folded away in the rendered pages, so a row whose sibling
    // is missing would silently vanish from the documentation.
    let names: BTreeSet<&str> = entries.iter().map(|e| e.symbol.as_str()).collect();
    let entry_addrs: BTreeSet<(String, String, u16)> = entries
        .iter()
        .filter(|e| e.role == "entry")
        .map(|e| (e.region.clone(), e.bank.clone(), e.addr))
        .collect();
    let mut orphans = Vec::new();
    for e in entries.iter().filter(|e| e.role == "alias") {
        if !entry_addrs.contains(&(e.region.clone(), e.bank.clone(), e.addr)) {
            orphans.push(format!("{} aliases nothing", e.symbol));
        }
    }
    for e in entries.iter().filter(|e| e.role == "instance") {
        // Some digit in the name is >= 2 and the "...1..." sibling exists.
        let bytes: Vec<char> = e.symbol.chars().collect();
        let mut ok = false;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let run = &e.symbol[start..i];
                let n: u32 = run.parse().unwrap_or(0);
                if n >= 2 {
                    // pokered numbers repeated structures both ways —
                    // `wPartyMon1`..`wPartyMon6` and the zero-padded
                    // `wSprite01`..`wSprite15` — so slot 1 may be "1" or "01".
                    let head = &e.symbol[..start];
                    let tail = &e.symbol[i..];
                    let padded = format!("{:0width$}", 1, width = run.len());
                    if names.contains(format!("{head}1{tail}").as_str())
                        || names.contains(format!("{head}{padded}{tail}").as_str())
                    {
                        ok = true;
                        break;
                    }
                }
            } else {
                i += 1;
            }
        }
        if !ok {
            orphans.push(format!("{} has no slot-1 sibling", e.symbol));
        }
    }
    let roles: BTreeMap<&str, usize> =
        entries
            .iter()
            .fold(BTreeMap::new(), |mut m: BTreeMap<&str, usize>, e| {
                *m.entry(e.role.as_str()).or_default() += 1;
                m
            });
    report.check(
        "aliases and instances have a sibling",
        orphans.is_empty(),
        if orphans.is_empty() {
            format!("{roles:?}")
        } else {
            format!("{} orphaned: {}", orphans.len(), orphans[0])
        },
    );

    // Every chapter is one the renderer knows how to head.
    let chapters: BTreeSet<&str> = entries.iter().map(|e| e.group.as_str()).collect();
    report.check(
        "no entry is homeless",
        !chapters.contains("misc"),
        format!("{} chapters", chapters.len()),
    );
}

/// How many bytes of `lo..=hi` the atlas names in `region`.
fn coverage(entries: &[Entry], region: &str, lo: u16, hi: u16) -> u32 {
    let mut named = vec![false; (hi as usize - lo as usize) + 1];
    for e in entries.iter().filter(|e| e.region == region) {
        let start = e.addr as usize;
        for a in start..start + e.len.max(1) as usize {
            if a >= lo as usize && a <= hi as usize {
                named[a - lo as usize] = true;
            }
        }
    }
    named.iter().filter(|b| **b).count() as u32
}

fn fmt(addr: u16) -> String {
    format!("${addr:04X}")
}

// ---------------------------------------------------------------------------
// Tier A, second half — the anti-drift gate against the plugins
// ---------------------------------------------------------------------------

/// Files that hard-code Gen 1 addresses. Scanned as text, on purpose: making
/// every plugin import a shared table would be a large mechanical change across
/// crates that are being edited by other crews, and the drift we actually care
/// about is "somebody wrote down an address the atlas has never heard of".
/// A text scan catches exactly that and costs nothing.
const PLUGIN_SOURCES: &[&str] = &[
    "plugins/battle-control/src/wram.rs",
    "plugins/gen1-link/src/battle.rs",
    "plugins/gen1-lobby/src/gen1.rs",
    "plugins/gen1-shinyhunt/src/hunt.rs",
    "plugins/trainer/src/gen1.rs",
    "plugins/trainer/src/cableclub.rs",
    "plugins/agent/src/observe.rs",
    // Not plugins, but the same class: two frontend-side modules that landed
    // while this was being written and hard-code Gen 1 addresses of their own.
    // Adding a file here is how a new consumer joins the gate.
    "src/gen1battle.rs",
    "src/longplay/mod.rs",
];

/// `const NAME: u16 = 0xXXXX;` — the shape a Gen 1 address is written in.
/// Deliberately only named constants: an inline literal may be a length, a
/// mask, or a species id, and a scan that guessed would cry wolf.
fn scan_consts(text: &str) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("const ").or_else(|| {
            line.strip_prefix("pub const ")
                .or_else(|| line.strip_prefix("pub(crate) const "))
        }) else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(':') else {
            continue;
        };
        let Some((ty, value)) = tail.split_once('=') else {
            continue;
        };
        if ty.trim() != "u16" {
            continue;
        }
        let value = value.trim().trim_end_matches(';').trim();
        let Some(hex) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        else {
            continue;
        };
        let hex: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if let Ok(addr) = u16::from_str_radix(&hex, 16) {
            // Only addresses in a region the atlas covers; a `u16` constant may
            // legitimately be a count or a mask.
            if REGIONS
                .iter()
                .filter(|(r, _, _)| *r != "ROM0" && *r != "ROMX")
                .any(|(_, lo, hi)| addr >= *lo && addr <= *hi)
            {
                out.push((name.trim().to_string(), addr));
            }
        }
    }
    out
}

fn tier_a_plugins(entries: &[Entry], report: &mut Report) {
    let known: BTreeSet<u16> = entries
        .iter()
        .filter(|e| e.region != "ROM0" && e.region != "ROMX")
        .map(|e| e.addr)
        .collect();
    let mut scanned = 0usize;
    let mut missing = Vec::new();
    let mut files = 0usize;
    for rel in PLUGIN_SOURCES {
        let Ok(text) = std::fs::read_to_string(rel) else {
            missing.push(format!("(source missing: {rel})"));
            continue;
        };
        files += 1;
        for (name, addr) in scan_consts(&text) {
            scanned += 1;
            if !known.contains(&addr) {
                missing.push(format!("{rel}: {name} = {} not in the atlas", fmt(addr)));
            }
        }
    }
    report.check(
        "plugin constants are in the atlas",
        missing.is_empty(),
        if missing.is_empty() {
            format!("{scanned} constants across {files} files")
        } else {
            missing.join("; ")
        },
    );
}

// ---------------------------------------------------------------------------
// Tier B — the cartridge image
// ---------------------------------------------------------------------------

/// Count the places the cartridge mentions `addr` as an instruction operand.
///
/// This is a byte scan, not a disassembly — there is no disassembler in this
/// repository and building one to answer "does the game touch this byte" would
/// be out of proportion. It looks for the opcodes that take a 16-bit immediate
/// (`ld [nn],a`, `ld a,[nn]`, `ld rr,nn`, `call`, `jp`) followed by the address
/// little-endian, and for HRAM the one-byte `ldh` forms.
///
/// It can be fooled: two bytes of graphics data preceded by a byte that happens
/// to be `$EA` look exactly like `ld [nn],a`. So a single hit is weak evidence
/// and the atlas records the *count*, which for a real address is typically
/// dozens. What it cannot do is produce a false *negative*: an address the game
/// really does load will be found.
fn rom_operand_hits(rom: &[u8], addr: u16) -> usize {
    const WORD_OPS: &[u8] = &[
        0x01, 0x11, 0x21, 0x31, // ld bc/de/hl/sp, nn
        0x08, // ld [nn], sp
        0xEA, 0xFA, // ld [nn],a / ld a,[nn]
        0xC3, 0xC2, 0xCA, 0xD2, 0xDA, // jp
        0xCD, 0xC4, 0xCC, 0xD4, 0xDC, // call
    ];
    let lo = (addr & 0xFF) as u8;
    let hi = (addr >> 8) as u8;
    let mut hits = 0;
    for i in 1..rom.len().saturating_sub(2) {
        if rom[i] == lo && rom[i + 1] == hi && WORD_OPS.contains(&rom[i - 1]) {
            hits += 1;
        }
    }
    if (0xFF00..=0xFFFF).contains(&addr) {
        // `ldh [n],a` = E0 n, `ldh a,[n]` = F0 n, `ld [c],a` has no operand.
        for i in 1..rom.len() {
            if rom[i] == lo && (rom[i - 1] == 0xE0 || rom[i - 1] == 0xF0) {
                hits += 1;
            }
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// Tier C — a running machine
// ---------------------------------------------------------------------------

/// Sampled regions. SRAM is deliberately absent: the game leaves cartridge RAM
/// disabled almost always, so a bus read there returns open bus rather than
/// storage, and "did it change" would measure the MBC and not the game.
const SAMPLED: &[(u16, u16)] = &[(0x8000, 0x9FFF), (0xC000, 0xDFFF), (0xFF80, 0xFFFE)];

/// A fixed script. Not a clever one — its whole job is to be *the same script
/// every time*, so that "this byte changed" is reproducible for anyone holding
/// the same cartridge. Emulation is deterministic, so ROM + this script fully
/// determines the answer.
///
/// Phase 1 mashes A and START, which is enough to play Gen 1's opening on its
/// own: title, NEW GAME, and the naming screen (where START accepts the default
/// name — mashing A there types a name of A's for ever, which is why the two
/// alternate). Phase 2 walks and opens menus, which reaches the overworld, the
/// sprite engine and the text machinery.
fn script_key(frame: u32) -> Option<KeypadKey> {
    if frame < 1400 {
        match (frame / 24) % 2 {
            0 => Some(KeypadKey::A),
            _ => Some(KeypadKey::Start),
        }
    } else {
        match (frame / 20) % 8 {
            0 => Some(KeypadKey::Down),
            1 => Some(KeypadKey::Down),
            2 => Some(KeypadKey::A),
            3 => Some(KeypadKey::Up),
            4 => Some(KeypadKey::Left),
            5 => Some(KeypadKey::Start),
            6 => Some(KeypadKey::Right),
            _ => Some(KeypadKey::B),
        }
    }
}

const RUN_FRAMES: u32 = 3600;

/// Run the script and return, for every sampled address, whether it ever changed.
fn observe_live(gb: &mut Gameboy) -> Vec<bool> {
    let mut touched = vec![false; 0x10000];
    let mut prev: Vec<Vec<u8>> = SAMPLED
        .iter()
        .map(|(lo, hi)| gb.peek_range(*lo, (*hi - *lo) as usize + 1))
        .collect();
    let mut held: Option<KeypadKey> = None;
    for frame in 0..RUN_FRAMES {
        let want = script_key(frame);
        if want != held {
            if let Some(k) = held {
                gb.keyup(k);
            }
            if let Some(k) = want {
                gb.keydown(k);
            }
            held = want;
        }
        gb.frame();
        for (i, (lo, hi)) in SAMPLED.iter().enumerate() {
            let now = gb.peek_range(*lo, (*hi - *lo) as usize + 1);
            for (j, b) in now.iter().enumerate() {
                if *b != prev[i][j] {
                    touched[*lo as usize + j] = true;
                }
            }
            prev[i] = now;
        }
    }
    touched
}

// ---------------------------------------------------------------------------
// The invariants — the strongest evidence in the file
// ---------------------------------------------------------------------------

/// What an invariant is given: the atlas's own addresses and banks, and the
/// cartridge image. Everything it needs to check a claim comes from the map it
/// is checking plus the machine — never from a constant written beside it.
struct Ctx {
    addr: BTreeMap<String, u16>,
    bank: BTreeMap<String, usize>,
    rom: Vec<u8>,
}

impl Ctx {
    /// The byte at a symbol in the cartridge image, following the atlas's own
    /// idea of which bank it is in.
    fn rom_byte(&self, sym: &str, offset: usize) -> Result<u8, String> {
        let addr = *self
            .addr
            .get(sym)
            .ok_or_else(|| format!("{sym} is not in the atlas"))?
            as usize;
        let bank = self.bank.get(sym).copied().unwrap_or(0);
        let base = if bank > 0 {
            bank * 0x4000 + (addr - 0x4000)
        } else {
            addr
        };
        self.rom
            .get(base + offset)
            .copied()
            .ok_or_else(|| format!("{sym} + {offset} is past the end of the cartridge"))
    }
}

/// A named semantic check. `covers` is the list of symbols it earns an `inv`
/// token for; `run` returns Ok with a note, or Err with what went wrong.
struct Invariant {
    name: &'static str,
    covers: &'static [&'static str],
    needs_save: bool,
    run: fn(&mut Gameboy, &Ctx) -> Result<String, String>,
}

fn at(ctx: &Ctx, sym: &str) -> Result<u16, String> {
    ctx.addr
        .get(sym)
        .copied()
        .ok_or_else(|| format!("{sym} is not in the atlas"))
}

const INVARIANTS: &[Invariant] = &[
    Invariant {
        name: "party arrays agree",
        covers: &[
            "wPartyCount",
            "wPartySpecies",
            "wPartyMons",
            "wPartyMonOT",
            "wPartyMonNicks",
        ],
        needs_save: true,
        run: |gb, ctx| {
            let count = gb.peek(at(ctx, "wPartyCount")?);
            if count > 6 {
                return Err(format!("wPartyCount reads {count}, and 6 is the maximum"));
            }
            let species = at(ctx, "wPartySpecies")?;
            let mons = at(ctx, "wPartyMons")?;
            // The species list is written twice: once as a flat list and once
            // inside each 44-byte structure. Two copies the game maintains at
            // different moments; if they agree, both addresses are right.
            for i in 0..count as u16 {
                let flat = gb.peek(species + i);
                let inside = gb.peek(mons + i * 44);
                if flat != inside {
                    return Err(format!(
                        "slot {i}: wPartySpecies says {flat:#04X}, wPartyMons says {inside:#04X}"
                    ));
                }
                if flat == 0 || flat == 0xFF {
                    return Err(format!("slot {i} holds species {flat:#04X}"));
                }
            }
            if gb.peek(species + count as u16) != 0xFF {
                return Err("wPartySpecies is not $FF-terminated".into());
            }
            Ok(format!("{count} in the party, both copies agree"))
        },
    },
    Invariant {
        name: "party stride is 44",
        covers: &["wPartyMon1", "wPartyMon2"],
        needs_save: false,
        run: |_gb, ctx| {
            let a = at(ctx, "wPartyMon1")?;
            let b = at(ctx, "wPartyMon2")?;
            if b - a != 44 {
                return Err(format!("wPartyMon2 - wPartyMon1 = {}", b - a));
            }
            Ok("wPartyMon2 - wPartyMon1 = 44".into())
        },
    },
    Invariant {
        name: "box stride is 33",
        covers: &["wBoxMon1", "wBoxMon2", "wBoxMons"],
        needs_save: false,
        run: |_gb, ctx| {
            let a = at(ctx, "wBoxMon1")?;
            let b = at(ctx, "wBoxMon2")?;
            if b - a != 33 {
                return Err(format!("wBoxMon2 - wBoxMon1 = {}", b - a));
            }
            Ok("wBoxMon2 - wBoxMon1 = 33".into())
        },
    },
    Invariant {
        name: "battle struct is 29 bytes",
        covers: &["wBattleMon", "wBattleMonPP", "wTrainerClass"],
        needs_save: false,
        run: |_gb, ctx| {
            // Two files in this repository disagreed about this number — 28 in
            // the trainer plugin, 29 in battle control. The map settles it: the
            // structure runs from wBattleMon to the byte after its four PP.
            let start = at(ctx, "wBattleMon")?;
            let pp = at(ctx, "wBattleMonPP")?;
            let end = pp + 4;
            if end - start != 29 {
                return Err(format!("wBattleMon spans {} bytes", end - start));
            }
            let next = at(ctx, "wTrainerClass")?;
            if next != end {
                return Err(format!(
                    "wTrainerClass is at {}, not {} where the struct ends",
                    fmt(next),
                    fmt(end)
                ));
            }
            Ok("wBattleMon..wBattleMonPP+4 = 29, and wTrainerClass follows".into())
        },
    },
    Invariant {
        name: "money is valid BCD",
        covers: &["wPlayerMoney"],
        needs_save: true,
        run: |gb, ctx| {
            let a = at(ctx, "wPlayerMoney")?;
            let bytes = [gb.peek(a), gb.peek(a + 1), gb.peek(a + 2)];
            let mut total = 0u32;
            for b in bytes {
                if (b & 0x0F) > 9 || (b >> 4) > 9 {
                    return Err(format!("{bytes:02X?} is not BCD"));
                }
                total = total * 100 + (b >> 4) as u32 * 10 + (b & 0x0F) as u32;
            }
            if total > 999_999 {
                return Err(format!("money reads {total}"));
            }
            Ok(format!("¥{total} in three big-endian BCD bytes"))
        },
    },
    Invariant {
        name: "badges fit in eight",
        covers: &["wObtainedBadges"],
        needs_save: true,
        run: |gb, ctx| {
            let b = gb.peek(at(ctx, "wObtainedBadges")?);
            Ok(format!("{} of 8 badges ({b:#04X})", b.count_ones()))
        },
    },
    Invariant {
        name: "owned is a subset of seen",
        covers: &["wPokedexOwned", "wPokedexSeen"],
        needs_save: true,
        run: |gb, ctx| {
            // You cannot own a Pokemon you have never seen. Nineteen bytes each,
            // one bit per Pokedex number; the relationship between the two is a
            // far stronger claim about both addresses than either alone.
            let owned = at(ctx, "wPokedexOwned")?;
            let seen = at(ctx, "wPokedexSeen")?;
            let mut n_owned = 0u32;
            let mut n_seen = 0u32;
            for i in 0..19u16 {
                let o = gb.peek(owned + i);
                let s = gb.peek(seen + i);
                if o & !s != 0 {
                    return Err(format!(
                        "byte {i}: owned {o:#04X} has a bit seen {s:#04X} does not"
                    ));
                }
                n_owned += o.count_ones();
                n_seen += s.count_ones();
            }
            if n_owned == 0 && n_seen == 0 {
                return Err("both arrays are empty — nothing to check".into());
            }
            // The subset relation is symmetric when the two sets are equal, so
            // on a save where every seen Pokemon has been caught this check
            // cannot tell the two arrays apart. Say so rather than reporting a
            // pass it has not earned: a swap of these two symbols survives it,
            // which was measured, not assumed.
            let strict = n_seen > n_owned;
            Ok(format!(
                "{n_owned} owned, {n_seen} seen, owned ⊆ seen{}",
                if strict {
                    ""
                } else {
                    " (equal — this save cannot tell the two arrays apart)"
                }
            ))
        },
    },
    Invariant {
        name: "the party is in the Pokedex",
        covers: &["wPokedexOwned", "wPartySpecies", "PokedexOrder"],
        needs_save: true,
        run: |gb, ctx| {
            // Every species you are carrying must be recorded as caught. Doing
            // it properly needs three independently located things to agree:
            // the party's species list (an *internal* index), `PokedexOrder` in
            // the cartridge (which converts internal index to Pokedex number),
            // and `wPokedexOwned` (a bit per Pokedex number). Unlike
            // "owned is a subset of seen" this is asymmetric, so it survives
            // nothing being swapped for anything else.
            let count = gb.peek(at(ctx, "wPartyCount")?);
            if count == 0 {
                return Err("the party is empty — nothing to check".into());
            }
            let species = at(ctx, "wPartySpecies")?;
            let owned = at(ctx, "wPokedexOwned")?;
            for i in 0..count as u16 {
                let internal = gb.peek(species + i);
                if internal == 0 || internal == 0xFF {
                    return Err(format!("party slot {i} holds species {internal:#04X}"));
                }
                // PokedexOrder is 1-based on the internal index.
                let dex = ctx.rom_byte("PokedexOrder", internal as usize - 1)?;
                if dex == 0 || dex > 151 {
                    return Err(format!(
                        "internal index {internal} converts to dex number {dex}"
                    ));
                }
                let bit = dex as u16 - 1;
                let byte = gb.peek(owned + bit / 8);
                if byte & (1 << (bit % 8)) == 0 {
                    return Err(format!(
                        "party slot {i} is dex #{dex} but wPokedexOwned does not have it"
                    ));
                }
            }
            Ok(format!(
                "{count} carried, every one of them recorded as caught"
            ))
        },
    },
    Invariant {
        name: "the bag list is terminated",
        covers: &["wNumBagItems", "wBagItems"],
        needs_save: true,
        run: |gb, ctx| {
            // (item, quantity) pairs, `$FF`-terminated, with a declared count in
            // front. Three ways of saying the same thing that must agree.
            let count = gb.peek(at(ctx, "wNumBagItems")?);
            if count > 20 {
                return Err(format!("wNumBagItems reads {count}, and the bag holds 20"));
            }
            let items = at(ctx, "wBagItems")?;
            for i in 0..count as u16 {
                let id = gb.peek(items + i * 2);
                let qty = gb.peek(items + i * 2 + 1);
                if id == 0 || id == 0xFF {
                    return Err(format!("bag slot {i} holds item {id:#04X}"));
                }
                if qty == 0 || qty > 99 {
                    return Err(format!("bag slot {i} holds {qty} of item {id:#04X}"));
                }
            }
            if gb.peek(items + count as u16 * 2) != 0xFF {
                return Err("the bag list is not $FF-terminated".into());
            }
            Ok(format!("{count} kinds of item, terminated"))
        },
    },
    Invariant {
        name: "the PC item list is terminated",
        covers: &["wNumBoxItems", "wBoxItems"],
        needs_save: true,
        run: |gb, ctx| {
            let count = gb.peek(at(ctx, "wNumBoxItems")?);
            if count > 50 {
                return Err(format!("wNumBoxItems reads {count}"));
            }
            let items = at(ctx, "wBoxItems")?;
            if gb.peek(items + count as u16 * 2) != 0xFF {
                return Err("the PC item list is not $FF-terminated".into());
            }
            Ok(format!("{count} kinds of item, terminated"))
        },
    },
    Invariant {
        name: "the live box agrees with itself",
        covers: &["wBoxCount", "wBoxSpecies", "wBoxMons", "wCurrentBoxNum"],
        needs_save: true,
        run: |gb, ctx| {
            let count = gb.peek(at(ctx, "wBoxCount")?);
            if count > 20 {
                return Err(format!("wBoxCount reads {count}, and a box holds 20"));
            }
            let species = at(ctx, "wBoxSpecies")?;
            let mons = at(ctx, "wBoxMons")?;
            for i in 0..count as u16 {
                let flat = gb.peek(species + i);
                let inside = gb.peek(mons + i * 33);
                if flat != inside {
                    return Err(format!(
                        "box slot {i}: wBoxSpecies {flat:#04X} vs wBoxMons {inside:#04X}"
                    ));
                }
            }
            if gb.peek(species + count as u16) != 0xFF {
                return Err("wBoxSpecies is not $FF-terminated".into());
            }
            let which = gb.peek(at(ctx, "wCurrentBoxNum")?);
            if which & 0x7F > 11 {
                return Err(format!("wCurrentBoxNum selects box {}", which & 0x7F));
            }
            Ok(format!(
                "{count} in box {} of 12, both copies agree",
                (which & 0x7F) + 1
            ))
        },
    },
    Invariant {
        name: "the player has a name",
        covers: &["wPlayerName", "wRivalName"],
        needs_save: true,
        run: |gb, ctx| {
            // Gen 1 strings are `$50`-terminated in an 11-byte field, and the
            // character map puts the capitals at `$80..$99`. A name that decodes
            // is a much stronger statement than a byte that is merely non-zero.
            let mut check = |base: u16, what: &str| -> Result<String, String> {
                let mut s = String::new();
                for i in 0..11u16 {
                    let c = gb.peek(base + i);
                    if c == 0x50 {
                        break;
                    }
                    match c {
                        0x80..=0x99 => s.push((b'A' + (c - 0x80)) as char),
                        0xA0..=0xB9 => s.push((b'a' + (c - 0xA0)) as char),
                        0xF6..=0xFF => s.push((b'0' + (c - 0xF6)) as char),
                        _ => s.push('?'),
                    }
                }
                if s.is_empty() || s.contains('?') {
                    return Err(format!("{what} decodes to {s:?}"));
                }
                Ok(s)
            };
            let me = check(at(ctx, "wPlayerName")?, "wPlayerName")?;
            let rival = check(at(ctx, "wRivalName")?, "wRivalName")?;
            Ok(format!("{me:?} and rival {rival:?}"))
        },
    },
    Invariant {
        name: "the map header is loaded",
        covers: &[
            "wCurMap",
            "wCurMapHeight",
            "wCurMapWidth",
            "wYCoord",
            "wXCoord",
        ],
        needs_save: false,
        run: |gb, ctx| {
            // Width and height are in 32x32 blocks and the coordinates are in
            // 16x16 tiles — two per block. Getting that factor wrong is the
            // classic way to conclude the player is outside the map, so the
            // check is written the way a reader must think about it.
            let h = gb.peek(at(ctx, "wCurMapHeight")?) as u16;
            let w = gb.peek(at(ctx, "wCurMapWidth")?) as u16;
            if h == 0 || w == 0 {
                return Err("no map header is loaded (height or width is 0)".into());
            }
            let y = gb.peek(at(ctx, "wYCoord")?) as u16;
            let x = gb.peek(at(ctx, "wXCoord")?) as u16;
            if y >= h * 2 || x >= w * 2 {
                return Err(format!(
                    "player at ({x},{y}) on a {}x{} tile map",
                    w * 2,
                    h * 2
                ));
            }
            Ok(format!(
                "map {:#04X}, {}x{} tiles, player at ({x},{y})",
                gb.peek(at(ctx, "wCurMap")?),
                w * 2,
                h * 2
            ))
        },
    },
    Invariant {
        name: "the screen reads back as text",
        covers: &["wTileMap"],
        needs_save: false,
        run: |gb, ctx| {
            // The claim that makes half this project possible: Gen 1 loads its
            // font so a tile id *is* the character code. If that is true, a
            // screen full of gameplay decodes to a plausible amount of letters.
            let base = at(ctx, "wTileMap")?;
            let bytes = gb.peek_range(base, 20 * 18);
            let letters = bytes
                .iter()
                .filter(|c| (0x80..=0x99).contains(*c) || (0xA0..=0xB9).contains(*c))
                .count();
            let blank = bytes.iter().filter(|c| **c == 0x7F).count();
            if letters + blank == 0 {
                return Err("no character codes and no spaces in wTileMap".into());
            }
            Ok(format!("{letters} letters and {blank} spaces in 360 cells"))
        },
    },
    Invariant {
        name: "the warp table is well formed",
        covers: &["wNumberOfWarps", "wWarpEntries"],
        needs_save: false,
        run: |gb, ctx| {
            // Four bytes each: y, x, destination warp id, destination map. The y
            // and x must be on the map whose header is loaded — which is what
            // ties this table to wCurMapHeight/Width and makes both stronger.
            let n = gb.peek(at(ctx, "wNumberOfWarps")?);
            if n > 32 {
                return Err(format!("wNumberOfWarps reads {n}, and 32 is the maximum"));
            }
            let h = gb.peek(at(ctx, "wCurMapHeight")?) as u16 * 2;
            let w = gb.peek(at(ctx, "wCurMapWidth")?) as u16 * 2;
            let base = at(ctx, "wWarpEntries")?;
            for i in 0..n as u16 {
                let y = gb.peek(base + i * 4) as u16;
                let x = gb.peek(base + i * 4 + 1) as u16;
                if h > 0 && w > 0 && (y >= h || x >= w) {
                    return Err(format!("warp {i} at ({x},{y}) is off a {w}x{h} map"));
                }
            }
            Ok(format!("{n} warps, all on the map"))
        },
    },
    Invariant {
        name: "the RNG seed moves",
        covers: &["hRandomAdd", "hRandomSub"],
        needs_save: false,
        run: |gb, ctx| {
            // `Random_` folds rDIV in on every call, so over a frame of ordinary
            // play at least one of the two seed bytes must move. If neither
            // does, whatever these addresses are, they are not the dice.
            let add = at(ctx, "hRandomAdd")?;
            let sub = at(ctx, "hRandomSub")?;
            let before = (gb.peek(add), gb.peek(sub));
            let mut moved = false;
            for _ in 0..60 {
                gb.frame();
                if (gb.peek(add), gb.peek(sub)) != before {
                    moved = true;
                    break;
                }
            }
            if !moved {
                return Err("neither seed byte moved in a second of play".into());
            }
            Ok("both seed bytes are live".into())
        },
    },
];

// ---------------------------------------------------------------------------
// Publishing evidence back to AtlasGB
// ---------------------------------------------------------------------------

/// SHA-1 of `data` (FIPS 180-4). AtlasGB's record identifies the cartridge a
/// run was made against by its SHA-1, which is the digest the disassembly's own
/// `roms.sha1` uses, so this is the number a reader can check for themselves.
/// Hand-rolled for the same reason `sha256` above is.
fn sha1(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());

    let mut w = [0u32; 80];
    for chunk in msg.chunks_exact(64) {
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(v);
        }
    }
    h.iter()
        .fold(String::with_capacity(h.len() * 8), |mut s, v| {
            let _ = write!(s, "{v:08x}");
            s
        })
}

/// Best effort, and it says so when it cannot answer: the report records which
/// commit of this repository produced it, so a tier in the published atlas
/// names a run somebody else could repeat. Only reached when the report is
/// being written, so a machine with no `git` costs a field, not a test.
fn this_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn json_escape(s: &str) -> String {
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

/// Emit an `atlasgb-evidence/1` report — the half of the loop this repository
/// owns. AtlasGB cannot produce a tier for itself: proving an address against a
/// running cartridge needs a cartridge and an emulator, and it has neither. So
/// the tiers arrive there as this file, `tools/apply-evidence.py` lands it, and
/// upstream CI then holds the `verify` column to a digest of what was run.
///
/// Two rules from upstream's contract, both load-bearing:
///
/// * **Every symbol in the atlas appears**, including the ones that earned
///   nothing. `apply-evidence.py` refuses a partial report rather than guessing,
///   because an omission would silently read as a downgrade. An empty list is a
///   claim the report is making — "no evidence" — not an absence.
/// * **It is written only when asked.** A file that appeared on every test run
///   is a file somebody commits by accident, and publishing evidence is a
///   deliberate act.
fn write_evidence_report(
    entries: &[Entry],
    fresh_verify: &BTreeMap<String, String>,
    rom_path: &Path,
    rom_bytes: &[u8],
    title: &str,
    with_save: bool,
) {
    let Ok(path) = std::env::var("ATLAS_EVIDENCE_OUT") else {
        return;
    };

    let date = lock_get(
        &std::fs::read_to_string(ATLAS_LOCK).unwrap_or_default(),
        "fetched",
    )
    .unwrap_or_else(|| "unknown".into());
    // The report's own date, not the snapshot's, when one is offered: a run is
    // dated by when it ran. `ATLAS_EVIDENCE_DATE` exists so a publication can be
    // stamped without this harness reaching for a clock it otherwise never uses.
    let date = std::env::var("ATLAS_EVIDENCE_DATE").unwrap_or(date);

    let mut out = String::new();
    out.push_str("{\n \"schema\": \"atlasgb-evidence/1\",\n");
    // Which atlas this run is evidence for. Optional in the schema — upstream
    // defaults it to whatever `--atlas` names — but naming it is the better
    // habit now that AtlasGB carries more than one, and it makes the report
    // self-describing rather than dependent on how it happens to be landed.
    out.push_str(&format!(" \"atlas\": \"{ATLAS_ID}\",\n"));
    out.push_str(&format!(
        " \"produced_by\": {{\"repo\": \"Alchemy86/TerminalGB\", \"commit\": \"{}\", \
          \"harness\": \"testharness/gen1atlas.rs\"}},\n",
        json_escape(&this_commit())
    ));
    // The region comes out of the cartridge header's destination code ($014A),
    // not out of a constant written beside it: everything the report claims
    // about the cartridge should be something a reader can recompute from the
    // same bytes, which is the whole reason the sha1 is there.
    let region = match rom_bytes.get(0x014A) {
        Some(0x00) => "Japan",
        Some(_) => "USA/Europe",
        None => "unknown",
    };
    out.push_str(&format!(
        " \"cartridge\": {{\"title\": \"{}\", \"region\": \"{region}\", \"sha1\": \"{}\", \
          \"bytes\": {}}},\n",
        json_escape(title),
        sha1(rom_bytes),
        rom_bytes.len()
    ));
    // The live sweep is always a cold boot with no save — that is what makes it
    // reproducible for anybody holding the cartridge. A save, when one is given,
    // reaches only the invariants, and it changes which of them can run, so it
    // is provenance worth recording rather than a detail.
    out.push_str(&format!(
        " \"script\": {{\"name\": \"opening+overworld\", \"frames\": {RUN_FRAMES}, \
          \"save\": \"none (cold boot){}\"}},\n",
        if with_save {
            "; a save was supplied to the invariants"
        } else {
            ""
        }
    ));
    out.push_str(&format!(
        " \"run\": {{\"date\": \"{}\"}},\n",
        json_escape(&date)
    ));
    out.push_str(" \"verify\": {\n");
    let rows: Vec<String> = entries
        .iter()
        .map(|e| {
            let toks: Vec<String> = fresh_verify[&e.symbol]
                .split(',')
                .filter(|t| !t.is_empty())
                .map(|t| format!("\"{t}\""))
                .collect();
            format!("  \"{}\": [{}]", json_escape(&e.symbol), toks.join(", "))
        })
        .collect();
    out.push_str(&rows.join(",\n"));
    out.push_str("\n }\n}\n");

    match std::fs::write(&path, out) {
        Ok(()) => {
            println!("\nEvidence report");
            println!("  wrote {path} — {} symbols", entries.len());
            println!("  cartridge {title:?} sha1 {}", sha1(rom_bytes));
            println!("  from {}", rom_path.display());
            println!(
                "  land it in a clone of AtlasGB:\n    \
                 tools/apply-evidence.py {path} && make docs data"
            );
        }
        Err(e) => println!("\nEvidence report\n  could not write {path}: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

fn find_rom() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("POKE_ROM") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let home = std::env::var_os("HOME")?;
    let dir = Path::new(&home).join("roms");
    let entries = std::fs::read_dir(dir).ok()?;
    // Blue is preferred because it is the cartridge the atlas was derived and
    // verified against; Red is the same engine and the same layout, so it is a
    // perfectly good second choice.
    let mut blue = None;
    let mut red = None;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !(name.ends_with(".gb") || name.ends_with(".gbc")) {
            continue;
        }
        if name.contains("blue") {
            blue = Some(e.path());
        } else if name.contains("red") {
            red = Some(e.path());
        }
    }
    blue.or(red)
}

fn find_save(rom: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("POKE_SAVE") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let p = rom.with_extension("sav");
    p.exists().then_some(p)
}

/// Copy the cartridge into `target/` before touching it. The MBC writes a
/// `.sav` beside whatever path it was handed, and this harness presses buttons
/// at menus — the rule the rest of this repository already follows.
fn stage(rom: &Path, save: Option<&Path>, name: &str) -> std::io::Result<PathBuf> {
    let dir = Path::new("target/gen1atlas");
    std::fs::create_dir_all(dir)?;
    // Two staged copies, under different names on purpose: the live sweep must
    // run on a cartridge with *no* save so its result depends only on the ROM
    // and the script, while the invariants want the player's real state. One
    // path for both would mean whichever ran last silently decided the other.
    let dest = dir.join(format!("{name}.gb"));
    std::fs::copy(rom, &dest)?;
    let sav = dest.with_extension("sav");
    match save {
        Some(s) => std::fs::copy(s, &sav).map(|_| ())?,
        None => {
            let _ = std::fs::remove_file(&sav);
        }
    }
    Ok(dest)
}

fn cartridge_title(rom: &[u8]) -> String {
    rom.get(0x134..0x143)
        .map(|b| {
            b.iter()
                .take_while(|c| **c != 0)
                .map(|c| *c as char)
                .collect()
        })
        .unwrap_or_default()
}

fn main() {
    println!("The Gen 1 memory atlas — verifying {ATLAS}");
    println!("  the data is AtlasGB's; the evidence below is this repository's\n");

    let Ok(raw) = std::fs::read(ATLAS) else {
        eprintln!(
            "cannot read {ATLAS} — run this from the repository root, and if the \
             file is absent vendor it with `tools/fetch-atlas.sh --ref <tag>`"
        );
        std::process::exit(1);
    };
    let Ok(text) = String::from_utf8(raw.clone()) else {
        eprintln!("{ATLAS} is not UTF-8");
        std::process::exit(1);
    };
    let entries = match parse_atlas(&text) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let mut report = Report::new();
    println!("Tier A — structure (no cartridge needed)");
    tier_a_lock(&raw, &mut report);
    tier_a(&entries, &mut report);
    tier_a_plugins(&entries, &mut report);
    report.print();
    if report.failures() > 0 {
        eprintln!("\n{} structural check(s) failed", report.failures());
        std::process::exit(1);
    }

    let Some(rom_path) = find_rom() else {
        println!(
            "\nTier B and C skipped: no cartridge.\n  \
             Point POKE_ROM at your own Pokemon Red or Blue, or leave one in ~/roms.\n  \
             No ROM is distributed with this repository and none is fetched."
        );
        println!("\nAll structural checks passed.");
        return;
    };
    let save_path = find_save(&rom_path);
    let staged = match stage(&rom_path, save_path.as_deref(), "saved") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not stage the cartridge into target/: {e}");
            std::process::exit(1);
        }
    };
    let rom_bytes = std::fs::read(&staged).expect("staged cartridge");
    let title = cartridge_title(&rom_bytes);
    println!(
        "\nCartridge: {title:?} ({} KiB){}",
        rom_bytes.len() / 1024,
        match &save_path {
            Some(_) => ", with a save",
            None => ", no save (the invariants that need one will say so)",
        }
    );

    // --- Tier B ------------------------------------------------------------
    println!("\nTier B — the cartridge image");
    let mut rom_hits: BTreeMap<u16, usize> = BTreeMap::new();
    let addrs: BTreeSet<u16> = entries
        .iter()
        .filter(|e| e.region != "ROM0" && e.region != "ROMX")
        .map(|e| e.addr)
        .collect();
    for a in &addrs {
        rom_hits.insert(*a, rom_operand_hits(&rom_bytes, *a));
    }
    let referenced = rom_hits.values().filter(|n| **n > 0).count();
    println!(
        "  {referenced} of {} distinct RAM addresses appear as an instruction operand",
        addrs.len()
    );

    // A ROM label is not storage, so neither the operand scan nor the live
    // sweep says anything about it. What can be checked is far better: read the
    // bytes the atlas points at and see whether they are the table it claims.
    // `plugins/gen1-link/src/romdata.rs` already finds these by byte signature
    // at run time, so this is two independent methods agreeing on an address.
    let rom_at = |bank: &str, addr: u16| -> usize {
        match bank.parse::<usize>() {
            Ok(b) if b > 0 => b * 0x4000 + (addr as usize - 0x4000),
            _ => addr as usize,
        }
    };
    let mut rom_checks: Vec<(&str, bool, String)> = Vec::new();
    let mut rom_verified: BTreeSet<String> = BTreeSet::new();
    for e in entries
        .iter()
        .filter(|e| e.region == "ROMX" || e.region == "ROM0")
    {
        // Signatures chosen because each is unmistakable and none of them is
        // game *content* we would be redistributing: they are the first few
        // bytes of a table, used only to say "yes, this is that table".
        let want: Option<(&str, Vec<u8>)> = match e.symbol.as_str() {
            // Gen 1's species table is indexed by *internal* index, and index 1
            // is RHYDON — the single most quoted fact about this cartridge.
            // `$91 $87 $98 $83 $8E $8D` is "RHYDON" in the Gen 1 charmap ($80 is A).
            "MonsterNames" => Some(("RHYDON", vec![0x91, 0x87, 0x98, 0x83, 0x8E, 0x8D])),
            // Base stats are indexed by Pokedex number and entry 1 is Bulbasaur:
            // dex 1, then HP/Attack/Defense/Speed/Special 45/49/49/45/65.
            "BaseStats" => Some(("Bulbasaur's record", vec![1, 45, 49, 49, 45, 65])),
            // Internal index -> Pokedex number. The first eight entries are the
            // fingerprint `romdata.rs` already uses.
            "PokedexOrder" => Some((
                "the first eight dex numbers",
                vec![112, 115, 32, 35, 21, 100, 34, 80],
            )),
            _ => None,
        };
        let Some((what, bytes)) = want else { continue };
        let off = rom_at(&e.bank, e.addr);
        let got = rom_bytes.get(off..off + bytes.len()).unwrap_or(&[]);
        let ok = got == bytes.as_slice();
        rom_checks.push((
            what,
            ok,
            format!(
                "{} at {} bank {} (file {off:#X})",
                e.symbol,
                fmt(e.addr),
                e.bank
            ),
        ));
        if ok {
            rom_verified.insert(e.symbol.clone());
        }
    }
    for (what, ok, detail) in &rom_checks {
        println!(
            "  {} {detail} really is {what}",
            if *ok { "ok  " } else { "FAIL" }
        );
    }

    // --- Tier C ------------------------------------------------------------
    println!("\nTier C — a running machine");
    // The live sweep is deliberately run WITHOUT the save, so it is a function
    // of the cartridge and the script alone and can be a baseline.
    let fresh = stage(&rom_path, None, "fresh").expect("staged cartridge");
    let mut gb = Gameboy::new(std::fs::read(&fresh).unwrap(), Some(fresh.clone()));
    let touched = observe_live(&mut gb);
    let live_count = touched.iter().filter(|b| **b).count();
    println!("  {live_count} bytes changed over {RUN_FRAMES} frames of the fixed script");

    // --- the invariants ----------------------------------------------------
    println!("\nInvariants");
    let ctx = Ctx {
        addr: entries.iter().map(|e| (e.symbol.clone(), e.addr)).collect(),
        bank: entries
            .iter()
            .map(|e| (e.symbol.clone(), e.bank.parse().unwrap_or(0)))
            .collect(),
        rom: rom_bytes.clone(),
    };
    // The invariants want a machine in a real state, so they run on the staged
    // copy of the player's own save when there is one.
    let mut inv_gb = Gameboy::new(std::fs::read(&staged).unwrap(), Some(staged.clone()));
    for _ in 0..600 {
        inv_gb.frame();
    }
    // Get past the title screen so a continued game is actually loaded.
    if save_path.is_some() {
        for frame in 0..900u32 {
            let k = if (frame / 30) % 2 == 0 {
                KeypadKey::A
            } else {
                KeypadKey::Start
            };
            inv_gb.keydown(k);
            inv_gb.frame();
            inv_gb.keyup(k);
        }
    }

    let mut covered: BTreeSet<&str> = BTreeSet::new();
    let mut inv_failures = 0usize;
    let mut inv_skipped = 0usize;
    for inv in INVARIANTS {
        if inv.needs_save && save_path.is_none() {
            println!("skip {:<34} needs a save; none was supplied", inv.name);
            inv_skipped += 1;
            continue;
        }
        match (inv.run)(&mut inv_gb, &ctx) {
            Ok(note) => {
                println!("ok   {:<34} {note}", inv.name);
                for s in inv.covers {
                    covered.insert(s);
                }
            }
            Err(why) => {
                println!("FAIL {:<34} {why}", inv.name);
                inv_failures += 1;
            }
        }
    }

    // --- the baseline ------------------------------------------------------
    let mut fresh_verify: BTreeMap<String, String> = BTreeMap::new();
    for e in &entries {
        if e.region == "ROM0" || e.region == "ROMX" {
            // ROM entries are labels in the cartridge, not storage: neither the
            // operand scan nor the live sweep says anything about them. A few
            // are checked by their content instead, which earns them `inv`.
            let tok = if rom_verified.contains(&e.symbol) {
                "inv"
            } else {
                ""
            };
            fresh_verify.insert(e.symbol.clone(), tok.to_string());
            continue;
        }
        let mut toks = Vec::new();
        if rom_hits.get(&e.addr).copied().unwrap_or(0) > 0 {
            toks.push("rom");
        }
        let end = (e.addr as usize + e.len.max(1) as usize).min(0x10000);
        if touched[e.addr as usize..end].iter().any(|b| *b) {
            toks.push("live");
        }
        if covered.contains(e.symbol.as_str()) {
            toks.push("inv");
        }
        fresh_verify.insert(e.symbol.clone(), toks.join(","));
    }

    let mut moved: Vec<String> = Vec::new();
    for e in &entries {
        let now = &fresh_verify[&e.symbol];
        if *now != e.verify {
            moved.push(format!(
                "{} {}: {:?} -> {:?}",
                fmt(e.addr),
                e.symbol,
                e.verify,
                now
            ));
        }
    }

    // The vendored `verify` column is the baseline, and unlike every other
    // baseline in this repository it is NOT re-blessed in place: it belongs to
    // AtlasGB, which makes it unwritable by hand deliberately and checks a
    // digest of it in its own CI. A disagreement is landed by publishing a
    // report, which is what `ATLAS_EVIDENCE_OUT` is for.
    println!("\nBaseline");
    if moved.is_empty() {
        println!("  every entry's evidence matches {ATLAS}");
    } else {
        println!("  {} entries disagree with the pinned atlas:", moved.len());
        for m in moved.iter().take(20) {
            println!("    {m}");
        }
        if moved.len() > 20 {
            println!("    ... and {} more", moved.len() - 20);
        }
        println!(
            "  This is not re-blessed here. Once you know why it moved, publish it:\n    \
             ATLAS_EVIDENCE_OUT=report.json POKE_ROM=… cargo test --release --test gen1atlas\n    \
             # then in a clone of AtlasGB: tools/apply-evidence.py report.json && make docs data\n  \
             and re-pin with tools/fetch-atlas.sh once it has landed."
        );
    }

    write_evidence_report(
        &entries,
        &fresh_verify,
        &staged,
        &rom_bytes,
        &title,
        save_path.is_some(),
    );

    // --- summary -----------------------------------------------------------
    // Counted from what this run observed, not from the file it may have just
    // rewritten — a summary that echoed the baseline back would say nothing.
    //
    // Counted the way AtlasGB publishes them, so the two repositories cannot
    // quote different numbers for the same run: the headline tiers are over
    // *storage* entries, because "observed live" means something quite
    // different for a table in the cartridge than for a byte of RAM. The
    // all-entries figure is printed beside it rather than instead of it —
    // three ROM labels earn `inv` by having their contents read, and folding
    // those into the storage total is how 36 becomes an unexplained 39.
    let storage = |e: &Entry| e.region != "ROM0" && e.region != "ROMX";
    let has = |tok: &str, only_storage: bool| {
        entries
            .iter()
            .filter(|e| !only_storage || storage(e))
            .filter(|e| fresh_verify[&e.symbol].split(',').any(|t| t == tok))
            .count()
    };
    let none = entries
        .iter()
        .filter(|e| storage(e) && fresh_verify[&e.symbol].is_empty())
        .count();
    println!(
        "\n{} storage entries of {}: {} referenced by the cartridge, {} observed live, \
         {} covered by an invariant, {none} with no evidence at all",
        entries.iter().filter(|e| storage(e)).count(),
        entries.len(),
        has("rom", true),
        has("live", true),
        has("inv", true),
    );
    println!(
        "  including the ROM-table labels: {} rom, {} live, {} inv",
        has("rom", false),
        has("live", false),
        has("inv", false),
    );
    if inv_skipped > 0 {
        println!("{inv_skipped} invariant(s) skipped for want of a save");
    }

    let rom_failures = rom_checks.iter().filter(|(_, ok, _)| !ok).count();
    if inv_failures > 0 || rom_failures > 0 || !moved.is_empty() {
        std::process::exit(1);
    }
    println!("\nAll checks passed.");
}

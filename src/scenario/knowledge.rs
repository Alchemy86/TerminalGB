//! **What the harness knows** — the sources a scenario may consult, and the
//! line between planning and cheating.
//!
//! # The line, stated once and enforced by where the code lives
//!
//! Reading a *reference* to plan with is legitimate: a player with a wiki open
//! is still playing. Reading *live memory* to decide is not, and the agent is
//! already barred from writing it. So this module holds only knowledge that is
//! **static** — true of the cartridge, not of this playthrough:
//!
//! * [`Atlas`] — AtlasGB's 2,898 evidenced Gen 1 addresses. What a *requirement*
//!   is expressed in terms of: `wObtainedBadges`, `wPartyCount`, `wEventFlags`.
//! * [`Guidance`] — what earlier runs worked out. A map graph, notes, a route.
//!   Emitted by a run, consumed by the next.
//!
//! The one source that is neither is [`BattleAdvice`], and it is quarantined
//! for exactly that reason: it reads the opponent's true stats out of the live
//! machine, which a player at the console cannot see. It is **off unless a
//! scenario asks for it**, and asking for it puts a line in the report. See
//! [`crate::scenario::rules`] on declared assistance.
//!
//! # Why the map graph is *learned* rather than transcribed
//!
//! A full Gen 1 route table exists in `pret/pokered` and could be copied out.
//! Two reasons not to. The licence boundary this repository keeps says facts
//! and formats yes, bulk game data no — a warp table is closer to data than to
//! a fact. And the captain's ask was for the agent to *build its own guidance*:
//! a graph a run discovered by walking through the doors is evidence the run
//! really went there, which a transcribed table is not. So [`Guidance`] starts
//! empty and grows, and a run that learns nothing says so.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use terminalgb_plugin_agent::json;

/// Bump when a field is renamed, retyped or removed. Additions are free.
pub const GUIDANCE_SCHEMA: &str = "gbguidance/1";

// -- the memory atlas ---------------------------------------------------------

/// One evidenced address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub symbol: String,
    pub addr: u16,
    pub len: u16,
    /// `WRAM0`, `HRAM`, …
    pub region: String,
    /// The subject grouping upstream files it under — `overworld`, `battle`.
    pub group: String,
    /// Evidence tiers: `rom` (named by an instruction), `live` (observed to
    /// change), `inv` (an invariant proved its meaning). An entry with none is
    /// a claim with nothing behind it and says so.
    pub verify: String,
    pub desc: String,
}

impl Entry {
    /// Is this address one an invariant has actually pinned?
    pub fn proven(&self) -> bool {
        self.verify.split(',').any(|t| t.trim() == "inv")
    }

    /// A one-line answer for an agent: what it is, where it is, how well known.
    pub fn summary(&self) -> String {
        let mut s = format!("{} at ${:04X}", self.symbol, self.addr);
        if self.len > 1 {
            s.push_str(&format!(" ({} bytes)", self.len));
        }
        if !self.verify.is_empty() {
            s.push_str(&format!(" [{}]", self.verify));
        }
        if !self.desc.is_empty() {
            // One sentence. The atlas's descriptions run to paragraphs and a
            // snapshot pays for every one of those characters on every call.
            let first = self
                .desc
                .split_once(". ")
                .map(|(a, _)| a.to_string())
                .unwrap_or_else(|| self.desc.clone());
            s.push_str(" — ");
            s.push_str(first.trim_end_matches('.'));
        }
        s
    }
}

/// The vendored AtlasGB snapshot, parsed.
///
/// **Loaded from disk at run time, never embedded.** Embedding 237 KB of TSV
/// would put it in the published crate and in the embedding build, which is a
/// contract this repository measures (`make embed`). A missing atlas is a
/// knowledge source that is simply unavailable, reported as such — never a
/// failure to start.
#[derive(Debug, Clone, Default)]
pub struct Atlas {
    entries: Vec<Entry>,
    by_symbol: BTreeMap<String, usize>,
    source: Option<PathBuf>,
}

impl Atlas {
    /// Where the vendored snapshot lives, relative to the repository root.
    pub const VENDORED: &'static str = "third_party/atlasgb/pokemon-rb/atlas.tsv";

    /// Find it: `$GB_ATLAS` first, then the vendored path walked up from the
    /// current directory, so a scout run from anywhere in the tree finds it.
    pub fn find() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("GB_ATLAS") {
            let p = PathBuf::from(p);
            return p.is_file().then_some(p);
        }
        let mut dir = std::env::current_dir().ok()?;
        loop {
            let c = dir.join(Self::VENDORED);
            if c.is_file() {
                return Some(c);
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    /// Load, or return `None` when there is nothing to load. Deliberately not a
    /// `Result`: an absent atlas is a normal state of the world.
    pub fn load() -> Option<Atlas> {
        let path = Self::find()?;
        let text = std::fs::read_to_string(&path).ok()?;
        let mut a = Atlas::parse(&text);
        a.source = Some(path);
        Some(a)
    }

    /// Parse the TSV. Columns:
    /// `region bank addr len symbol role sect group verify desc`.
    ///
    /// Rows whose `role` is `gap` or `free` describe *unreached* space and are
    /// dropped: they are how upstream proves its coverage adds up, and they are
    /// not addresses anybody can look up.
    pub fn parse(text: &str) -> Atlas {
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if i == 0 || line.trim().is_empty() {
                continue; // header
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 9 {
                continue;
            }
            let role = f[5];
            if role == "gap" || role == "free" || f[4].is_empty() {
                continue;
            }
            let Some(addr) = parse_hex(f[2]) else {
                continue;
            };
            entries.push(Entry {
                symbol: f[4].to_string(),
                addr,
                len: f[3].parse().unwrap_or(1),
                region: f[0].to_string(),
                group: f[7].to_string(),
                verify: f[8].to_string(),
                desc: f.get(9).copied().unwrap_or("").to_string(),
            });
        }
        let mut by_symbol = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            // First wins: an `entry` row precedes its `alias` rows upstream, and
            // the entry is the canonical name for an address.
            by_symbol.entry(e.symbol.clone()).or_insert(i);
        }
        Atlas {
            entries,
            by_symbol,
            source: None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// Exact symbol lookup.
    pub fn symbol(&self, name: &str) -> Option<&Entry> {
        self.by_symbol.get(name).map(|i| &self.entries[*i])
    }

    /// What lives at an address — the entry whose span contains it.
    pub fn at(&self, addr: u16) -> Option<&Entry> {
        self.entries
            .iter()
            .filter(|e| addr >= e.addr && addr < e.addr.saturating_add(e.len.max(1)))
            // The tightest span wins: `wPartyMon1` contains `wPartyMon1HP`.
            .min_by_key(|e| e.len)
    }

    /// Case-insensitive substring search over symbol, group and description.
    /// Symbol matches rank above description matches, because somebody typing
    /// `badges` wants `wObtainedBadges` before a paragraph that mentions one.
    pub fn search(&self, needle: &str, limit: usize) -> Vec<&Entry> {
        let n = needle.to_ascii_lowercase();
        if n.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(u8, &Entry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let sym = e.symbol.to_ascii_lowercase();
                if sym == n {
                    Some((0, e))
                } else if sym.contains(&n) {
                    Some((1, e))
                } else if e.group.to_ascii_lowercase().contains(&n) {
                    Some((2, e))
                } else if e.desc.to_ascii_lowercase().contains(&n) {
                    Some((3, e))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by_key(|(rank, e)| (*rank, e.symbol.len(), e.addr));
        hits.into_iter().take(limit).map(|(_, e)| e).collect()
    }
}

fn parse_hex(s: &str) -> Option<u16> {
    let t = s.trim();
    let h = t.strip_prefix('$').or_else(|| t.strip_prefix("0x"))?;
    u16::from_str_radix(h, 16).ok()
}

// -- learned guidance ---------------------------------------------------------

/// A door that was walked through, and where it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub from: u8,
    pub to: u8,
    /// Where on `from` the exit was standing when it fired.
    pub x: u8,
    pub y: u8,
    /// How many times a run has actually used it. Evidence, not a guess.
    pub uses: u32,
}

/// What runs have worked out about a cartridge, and can hand to the next run.
///
/// This is the captain's *"build its own rules and AI for guidance"* in file
/// form. A run emits it; a later run of the same or a neighbouring scenario is
/// given it in its snapshot, so a solved segment is cheap the second time.
///
/// Everything here is **derived from play**: a link exists because a console
/// walked through that door. Nothing is transcribed from a disassembly, so the
/// file is honest about how much has actually been explored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guidance {
    /// Cartridge this was learned on, so it cannot be applied to another.
    pub rom_title: String,
    pub links: Vec<Link>,
    /// Free-text notes keyed by map id — what a run wants its successor told.
    pub notes: BTreeMap<u8, Vec<String>>,
    /// Notes that are not about any one map.
    pub general: Vec<String>,
}

impl Guidance {
    pub fn is_empty(&self) -> bool {
        self.links.is_empty() && self.notes.is_empty() && self.general.is_empty()
    }

    /// Record a transition. Idempotent per door: a second use bumps `uses`.
    pub fn observe_link(&mut self, from: u8, to: u8, x: u8, y: u8) {
        if let Some(l) = self
            .links
            .iter_mut()
            .find(|l| l.from == from && l.to == to && l.x == x && l.y == y)
        {
            l.uses += 1;
            return;
        }
        self.links.push(Link {
            from,
            to,
            x,
            y,
            uses: 1,
        });
    }

    pub fn note(&mut self, map: u8, text: &str) {
        let v = self.notes.entry(map).or_default();
        if !v.iter().any(|n| n == text) {
            v.push(text.to_string());
        }
    }

    /// Every map reachable in one step from `map`.
    pub fn exits_from(&self, map: u8) -> Vec<&Link> {
        self.links.iter().filter(|l| l.from == map).collect()
    }

    /// Breadth-first route `from` → `to` over what has been walked, as a list
    /// of map ids beginning with `from`. `None` when no known door connects
    /// them — which is the honest answer for a graph built out of experience.
    pub fn route(&self, from: u8, to: u8) -> Option<Vec<u8>> {
        if from == to {
            return Some(vec![from]);
        }
        let mut seen: BTreeMap<u8, u8> = BTreeMap::new();
        let mut queue = std::collections::VecDeque::from([from]);
        seen.insert(from, from);
        while let Some(m) = queue.pop_front() {
            for l in self.exits_from(m) {
                if seen.contains_key(&l.to) {
                    continue;
                }
                seen.insert(l.to, m);
                if l.to == to {
                    let mut path = vec![to];
                    let mut cur = to;
                    while cur != from {
                        cur = seen[&cur];
                        path.push(cur);
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(l.to);
            }
        }
        None
    }

    /// Fold another run's discoveries in. Use counts add, so a link two runs
    /// found is better evidenced than one that was seen once.
    pub fn merge(&mut self, other: &Guidance) {
        if self.rom_title.is_empty() {
            self.rom_title = other.rom_title.clone();
        }
        for l in &other.links {
            match self
                .links
                .iter_mut()
                .find(|m| m.from == l.from && m.to == l.to && m.x == l.x && m.y == l.y)
            {
                Some(m) => m.uses += l.uses,
                None => self.links.push(l.clone()),
            }
        }
        for (map, notes) in &other.notes {
            for n in notes {
                self.note(*map, n);
            }
        }
        for n in &other.general {
            if !self.general.iter().any(|g| g == n) {
                self.general.push(n.clone());
            }
        }
    }

    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", GUIDANCE_SCHEMA);
        w.str_field("rom_title", &self.rom_title);
        w.arr_field("links");
        for l in &self.links {
            w.begin_obj();
            w.num_field("from", l.from as f64);
            w.str_field("from_name", crate::longplay::map_name(l.from));
            w.num_field("to", l.to as f64);
            w.str_field("to_name", crate::longplay::map_name(l.to));
            w.num_field("x", l.x as f64);
            w.num_field("y", l.y as f64);
            w.num_field("uses", l.uses as f64);
            w.end_obj();
        }
        w.end_arr();
        w.arr_field("notes");
        for (map, notes) in &self.notes {
            for n in notes {
                w.begin_obj();
                w.num_field("map", *map as f64);
                w.str_field("map_name", crate::longplay::map_name(*map));
                w.str_field("note", n);
                w.end_obj();
            }
        }
        w.end_arr();
        w.arr_field("general");
        for n in &self.general {
            w.str_item(n);
        }
        w.end_arr();
        w.end_obj();
        w.finish()
    }

    pub fn from_json(text: &str) -> Result<Guidance, String> {
        let v =
            json::parse(text).map_err(|e| format!("guidance is not JSON: {}", e.0))?;
        let schema = v.str_field("schema").unwrap_or("");
        if schema != GUIDANCE_SCHEMA {
            return Err(format!(
                "guidance schema is {schema:?}, this build reads {GUIDANCE_SCHEMA:?}"
            ));
        }
        let mut g = Guidance {
            rom_title: v.str_field("rom_title").unwrap_or("").to_string(),
            ..Default::default()
        };
        for l in v.get("links").and_then(|a| a.as_array()).unwrap_or(&[]) {
            g.links.push(Link {
                from: l.u64_field("from").unwrap_or(0) as u8,
                to: l.u64_field("to").unwrap_or(0) as u8,
                x: l.u64_field("x").unwrap_or(0) as u8,
                y: l.u64_field("y").unwrap_or(0) as u8,
                uses: l.u64_field("uses").unwrap_or(1) as u32,
            });
        }
        for n in v.get("notes").and_then(|a| a.as_array()).unwrap_or(&[]) {
            let map = n.u64_field("map").unwrap_or(0) as u8;
            if let Some(t) = n.str_field("note") {
                g.note(map, t);
            }
        }
        for n in v.get("general").and_then(|a| a.as_array()).unwrap_or(&[]) {
            if let Some(s) = n.as_str() {
                g.general.push(s.to_string());
            }
        }
        Ok(g)
    }

    pub fn load(path: &Path) -> Result<Guidance, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Guidance::from_json(&text)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).ok();
        }
        std::fs::write(path, self.to_json())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

// -- what the agent is told ---------------------------------------------------

/// The knowledge a scenario has switched on, assembled once and consulted per
/// decision.
#[derive(Debug, Default)]
pub struct Knowledge {
    pub atlas: Option<Atlas>,
    pub guidance: Guidance,
    /// Names of the sources actually available, for the report. A scenario that
    /// *asked* for the atlas and did not get one must not read as one that
    /// never wanted it.
    pub sources: Vec<String>,
    pub missing: Vec<String>,
}

impl Knowledge {
    /// Notes to hand the agent for its current position, newest knowledge
    /// first. Bounded on purpose: this is paid for on every call.
    pub fn notes_for(&self, map: u8, goal_map: Option<u8>) -> Vec<String> {
        let mut out = Vec::new();
        for n in &self.general {
            out.push(n.clone());
        }
        if let Some(notes) = self.guidance.notes.get(&map) {
            for n in notes {
                out.push(format!("Here ({}): {n}", crate::longplay::map_name(map)));
            }
        }
        if let Some(goal) = goal_map {
            match self.guidance.route(map, goal) {
                Some(path) if path.len() > 1 => {
                    let names: Vec<String> = path
                        .iter()
                        .map(|m| {
                            let n = crate::longplay::map_name(*m);
                            if n.is_empty() {
                                format!("map {m}")
                            } else {
                                n.to_string()
                            }
                        })
                        .collect();
                    out.push(format!(
                        "A route earlier runs actually walked: {}. The next step \
                         is {}.",
                        names.join(" → "),
                        names[1]
                    ));
                    if let Some(l) = self
                        .guidance
                        .exits_from(map)
                        .into_iter()
                        .find(|l| l.to == path[1])
                    {
                        out.push(format!(
                            "The door to {} was at ({}, {}) — walk onto it and \
                             then one more step in the direction it faces.",
                            names[1], l.x, l.y
                        ));
                    }
                }
                Some(_) => {}
                None => {
                    if !self.guidance.links.is_empty() {
                        out.push(
                            "No earlier run has walked from here to the goal, so \
                             there is no route to follow — explore."
                                .to_string(),
                        );
                    }
                }
            }
        }
        out
    }
}

impl std::ops::Deref for Knowledge {
    type Target = Guidance;
    fn deref(&self) -> &Guidance {
        &self.guidance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "region\tbank\taddr\tlen\tsymbol\trole\tsect\tgroup\tverify\tdesc\n\
        WRAM0\t-\t$D35E\t1\twCurMap\tentry\tMain Data\toverworld\trom,live,inv\tThe map you are standing on. More prose.\n\
        WRAM0\t-\t$D356\t1\twObtainedBadges\tentry\tMain Data\tprogress\trom,live,inv\tOne bit per badge.\n\
        WRAM0\t-\t$D163\t1\twPartyCount\tentry\tMain Data\tparty\trom,live\tHow many are in the party.\n\
        WRAM0\t-\t$D164\t7\twPartySpecies\tentry\tMain Data\tparty\trom\tSpecies list, $FF terminated.\n\
        WRAM0\t-\t$DF00\t255\t\tfree\tStack\t-\t\tunreached\n";

    #[test]
    fn the_atlas_answers_by_symbol_and_by_address() {
        let a = Atlas::parse(SAMPLE);
        assert_eq!(a.len(), 4, "the `free` row is not an address to look up");
        assert_eq!(a.symbol("wCurMap").unwrap().addr, 0xD35E);
        // Inside a multi-byte span, not just at its head.
        assert_eq!(a.at(0xD167).unwrap().symbol, "wPartySpecies");
        assert!(a.at(0x8000).is_none());
    }

    /// A summary is what an agent is charged tokens for, so it is one sentence,
    /// and it carries the evidence tier rather than presenting every claim as
    /// equally certain.
    #[test]
    fn a_summary_is_one_sentence_and_names_its_evidence() {
        let a = Atlas::parse(SAMPLE);
        let s = a.symbol("wCurMap").unwrap().summary();
        assert!(s.contains("$D35E"), "{s}");
        assert!(s.contains("rom,live,inv"), "{s}");
        assert!(!s.contains("More prose"), "only the first sentence: {s}");
        assert!(a.symbol("wCurMap").unwrap().proven());
        assert!(!a.symbol("wPartySpecies").unwrap().proven());
    }

    #[test]
    fn search_puts_the_symbol_before_the_paragraph_that_mentions_it() {
        let a = Atlas::parse(SAMPLE);
        let hits = a.search("badges", 5);
        assert_eq!(hits[0].symbol, "wObtainedBadges");
        assert!(a.search("", 5).is_empty());
    }

    /// The graph is evidence: a route exists because somebody walked it.
    #[test]
    fn a_route_only_exists_over_doors_a_run_actually_used() {
        let mut g = Guidance::default();
        g.observe_link(0x26, 0x25, 7, 1); // bedroom → downstairs
        g.observe_link(0x25, 0x00, 2, 7); // downstairs → Pallet
        assert_eq!(g.route(0x26, 0x00), Some(vec![0x26, 0x25, 0x00]));
        assert_eq!(g.route(0x26, 0x0C), None, "nobody has been there");
        assert_eq!(g.route(0x26, 0x26), Some(vec![0x26]));
    }

    #[test]
    fn using_a_door_twice_is_evidence_not_a_duplicate() {
        let mut g = Guidance::default();
        g.observe_link(1, 2, 3, 4);
        g.observe_link(1, 2, 3, 4);
        assert_eq!(g.links.len(), 1);
        assert_eq!(g.links[0].uses, 2);
    }

    #[test]
    fn guidance_round_trips_through_its_own_json() {
        let mut g = Guidance {
            rom_title: "POKEMON BLUE".into(),
            ..Default::default()
        };
        g.observe_link(0x26, 0x25, 7, 1);
        g.note(0x26, "The stairs are at (7,1).");
        g.general.push("Press START to finish a name.".into());
        let back = Guidance::from_json(&g.to_json()).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn a_wrong_schema_is_refused_rather_than_half_read() {
        assert!(Guidance::from_json("{\"schema\":\"gbguidance/9\"}").is_err());
    }

    /// Merging is how one run's discoveries reach the next, and use counts must
    /// add or a link two runs found would look no better evidenced than one.
    #[test]
    fn merging_adds_evidence_rather_than_duplicating_it() {
        let mut a = Guidance::default();
        a.observe_link(1, 2, 3, 4);
        let mut b = Guidance::default();
        b.observe_link(1, 2, 3, 4);
        b.observe_link(2, 3, 5, 6);
        a.merge(&b);
        assert_eq!(a.links.len(), 2);
        assert_eq!(a.links[0].uses, 2);
    }

    /// The point of the whole module: an agent standing in the bedroom and
    /// asked for Pallet Town is told the way, in words, with the door.
    #[test]
    fn the_agent_is_handed_a_route_it_can_act_on() {
        let mut k = Knowledge::default();
        k.guidance.observe_link(0x26, 0x25, 7, 1);
        k.guidance.observe_link(0x25, 0x00, 2, 7);
        let notes = k.notes_for(0x26, Some(0x00));
        let joined = notes.join(" | ");
        assert!(joined.contains("→"), "{joined}");
        assert!(joined.contains("(7, 1)"), "the door itself: {joined}");
    }

    #[test]
    fn an_unexplored_goal_is_said_to_be_unexplored_not_guessed_at() {
        let mut k = Knowledge::default();
        k.guidance.observe_link(0x26, 0x25, 7, 1);
        let notes = k.notes_for(0x26, Some(0x0C)).join(" | ");
        assert!(notes.contains("explore"), "{notes}");
    }
}

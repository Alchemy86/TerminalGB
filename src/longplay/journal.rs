//! Durable run state: an append-only event log, and a checkpoint a restart can
//! pick up from.
//!
//! # Why a run needs durable state at all
//!
//! A model's context does not span seventy hours of game time. Neither does a
//! process: a long run is exactly the kind of thing that gets killed by a
//! reboot, an OOM, a closed laptop lid or a `^C` at hour four. Everything the
//! run has learned that is not in the cartridge — which maps it has seen, which
//! milestones it has already reported, how much budget it has spent, how many
//! times it has been resumed — lives here.
//!
//! # Two files, and the split matters
//!
//! * **`events.ndjson`** — append-only, one JSON object per line, flushed on
//!   every write. This is the record: it is never rewritten, so a crash can
//!   truncate at most the line being written and everything before it survives.
//! * **`checkpoint.json` + `checkpoint.gbss`** — the resume point, rewritten
//!   periodically and **atomically** (tmp → rename, the same rule
//!   `src/mbc/battery.rs` follows for exactly the same reason: a half-written
//!   checkpoint is worse than an old one).
//!
//! # A resume is a rewind, and it is declared
//!
//! Loading a save state rewinds the game to the checkpoint. That is a
//! legitimate thing for a crash-resume to do and an *illegitimate* thing to do
//! to make an agent look competent, so the two are kept apart by making it
//! impossible to do quietly: the checkpoint counts resumes, the count goes in
//! the report, and [`super::progress::MilestoneKind::Resumed`] is `Major` so it
//! is pushed like a badge. There is no other path in this module that writes to
//! the machine at all.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use terminalgb_plugin_agent::json;

/// Bump when a checkpoint field is renamed, retyped or removed. Additions are
/// free; a mismatch is refused rather than half-read.
pub const CHECKPOINT_SCHEMA: &str = "gblongplay-checkpoint/1";
/// The event stream's schema. Additions are free.
pub const EVENT_SCHEMA: &str = "gblongplay/1";

/// Everything a restart needs that is not in the save state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Checkpoint {
    /// Emulated frame the state was taken at.
    pub frame: u64,
    /// The game's own play clock, in seconds.
    pub play_seconds: u32,
    /// Wall-clock seconds spent across **every** session of this run.
    pub wall_seconds: u64,
    /// How many times this run has been resumed. Zero on a fresh run.
    pub resumes: u32,
    /// Model calls made across every session.
    pub calls: u32,
    /// Decisions taken across every session.
    pub decisions: u64,
    /// Milestones already reported, so a resume does not replay them.
    pub milestones: u64,
    /// Reported tokens and spend across every session.
    pub tokens: u64,
    pub usd: f64,
    /// Map ids already visited.
    pub visited: BTreeSet<u8>,
    /// Badges already reported.
    pub badges: u8,
    /// Best level seen per party slot.
    pub levels: Vec<u8>,
    /// Best Pokédex-owned count seen.
    pub owned: u16,
    /// Best progress score reached, so a resumed run does not immediately
    /// declare a wall it had already cleared.
    pub wall_best: u64,
    /// The objective, so a resume cannot silently change what the run is for.
    pub objective: String,
    /// Cartridge title, checked on resume.
    pub rom_title: String,
}

impl Checkpoint {
    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", CHECKPOINT_SCHEMA);
        w.num_field("frame", self.frame as f64);
        w.num_field("play_seconds", self.play_seconds as f64);
        w.num_field("wall_seconds", self.wall_seconds as f64);
        w.num_field("resumes", self.resumes as f64);
        w.num_field("calls", self.calls as f64);
        w.num_field("decisions", self.decisions as f64);
        w.num_field("milestones", self.milestones as f64);
        w.num_field("tokens", self.tokens as f64);
        w.num_field("usd", self.usd);
        w.num_field("badges", self.badges as f64);
        w.num_field("owned", self.owned as f64);
        w.num_field("wall_best", self.wall_best as f64);
        w.str_field("objective", &self.objective);
        w.str_field("rom_title", &self.rom_title);
        w.arr_field("visited");
        for m in &self.visited {
            w.num_item(*m as f64);
        }
        w.end_arr();
        w.arr_field("levels");
        for l in &self.levels {
            w.num_item(*l as f64);
        }
        w.end_arr();
        w.end_obj();
        w.finish()
    }

    pub fn from_json(text: &str) -> Result<Checkpoint, String> {
        let v =
            json::parse(text).map_err(|e| format!("checkpoint is not JSON: {}", e.0))?;
        let schema = v.str_field("schema").unwrap_or("");
        if schema != CHECKPOINT_SCHEMA {
            return Err(format!(
                "checkpoint schema is {schema:?}, this build writes {CHECKPOINT_SCHEMA:?}"
            ));
        }
        let n = |k: &str| v.u64_field(k).unwrap_or(0);
        Ok(Checkpoint {
            frame: n("frame"),
            play_seconds: n("play_seconds") as u32,
            wall_seconds: n("wall_seconds"),
            resumes: n("resumes") as u32,
            calls: n("calls") as u32,
            decisions: n("decisions"),
            milestones: n("milestones"),
            tokens: n("tokens"),
            usd: v.get("usd").and_then(|u| u.as_f64()).unwrap_or(0.0),
            badges: n("badges") as u8,
            owned: n("owned") as u16,
            wall_best: n("wall_best"),
            objective: v.str_field("objective").unwrap_or("").to_string(),
            rom_title: v.str_field("rom_title").unwrap_or("").to_string(),
            visited: v
                .get("visited")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64())
                        .map(|x| x as u8)
                        .collect()
                })
                .unwrap_or_default(),
            levels: v
                .get("levels")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64())
                        .map(|x| x as u8)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// The run directory: where the log, the checkpoint and the report live.
pub struct Journal {
    dir: PathBuf,
    events: Option<std::io::BufWriter<std::fs::File>>,
    lines: u64,
}

impl Journal {
    /// Open (creating) a run directory. Appends to any existing event log, so
    /// a resumed run's history stays in one file.
    pub fn open(dir: &Path) -> std::io::Result<Journal> {
        std::fs::create_dir_all(dir)?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.ndjson"))?;
        Ok(Journal {
            dir: dir.to_path_buf(),
            events: Some(std::io::BufWriter::new(f)),
            lines: 0,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Lines written this session.
    pub fn lines(&self) -> u64 {
        self.lines
    }

    /// Append one already-encoded JSON object.
    ///
    /// Flushed immediately. A long run is exactly the process that gets killed
    /// without warning, and an event log that only reaches disk at exit is a
    /// log that is empty precisely when it is needed.
    pub fn write(&mut self, line: &str) {
        if let Some(f) = self.events.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
            self.lines += 1;
        }
    }

    /// Write the checkpoint pair atomically.
    ///
    /// The save state is written first: a checkpoint JSON pointing at a state
    /// that does not exist yet is unresumable, whereas a fresh state beside an
    /// older JSON simply resumes from the older point.
    pub fn checkpoint(&mut self, cp: &Checkpoint, state: &[u8]) -> std::io::Result<()> {
        atomic_write(&self.dir.join("checkpoint.gbss"), state)?;
        atomic_write(&self.dir.join("checkpoint.json"), cp.to_json().as_bytes())?;
        Ok(())
    }

    /// Read a checkpoint back, with its save state. `Ok(None)` means "no
    /// checkpoint here", which is not an error — it is a fresh run.
    pub fn read_checkpoint(dir: &Path) -> Result<Option<(Checkpoint, Vec<u8>)>, String> {
        let jpath = dir.join("checkpoint.json");
        if !jpath.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&jpath)
            .map_err(|e| format!("cannot read {}: {e}", jpath.display()))?;
        let cp = Checkpoint::from_json(&text)?;
        let spath = dir.join("checkpoint.gbss");
        let state = std::fs::read(&spath)
            .map_err(|e| format!("cannot read {}: {e}", spath.display()))?;
        Ok(Some((cp, state)))
    }

    /// Write the rendered report beside the log.
    pub fn write_report(&self, name: &str, body: &str) -> std::io::Result<PathBuf> {
        let p = self.dir.join(name);
        atomic_write(&p, body.as_bytes())?;
        Ok(p)
    }
}

/// tmp → rename. A truncated checkpoint is worse than a stale one.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Checkpoint {
        Checkpoint {
            frame: 123_456,
            play_seconds: 4_000,
            wall_seconds: 900,
            resumes: 2,
            calls: 41,
            decisions: 40,
            milestones: 9,
            tokens: 120_000,
            usd: 0.4231,
            visited: [0u8, 1, 12, 0x28].into_iter().collect(),
            badges: 0b11,
            levels: vec![14, 9],
            owned: 5,
            wall_best: 8_100,
            objective: "Reach Pewter City".into(),
            rom_title: "POKEMON BLUE".into(),
        }
    }

    #[test]
    fn a_checkpoint_round_trips_every_field() {
        let cp = sample();
        let back = Checkpoint::from_json(&cp.to_json()).expect("parse");
        assert_eq!(cp, back);
    }

    #[test]
    fn a_checkpoint_from_another_schema_is_refused_not_half_read() {
        let bad = r#"{"schema":"something-else/9","frame":7}"#;
        let e = Checkpoint::from_json(bad).unwrap_err();
        assert!(e.contains("schema"), "{e}");
        assert!(Checkpoint::from_json("not json at all").is_err());
    }

    #[test]
    fn the_event_log_is_appended_to_so_a_resume_keeps_its_history() {
        let dir = std::env::temp_dir().join(format!(
            "gb-longplay-journal-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut j = Journal::open(&dir).expect("open");
            j.write(r#"{"ev":"one"}"#);
        }
        {
            let mut j = Journal::open(&dir).expect("reopen");
            j.write(r#"{"ev":"two"}"#);
            assert_eq!(j.lines(), 1, "counts this session only");
        }
        let body = std::fs::read_to_string(dir.join("events.ndjson")).unwrap();
        assert_eq!(body.lines().count(), 2, "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checkpoint_survives_a_round_trip_through_the_filesystem() {
        let dir = std::env::temp_dir().join(format!(
            "gb-longplay-cp-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut j = Journal::open(&dir).expect("open");
        assert!(
            Journal::read_checkpoint(&dir).expect("read").is_none(),
            "a fresh run has no checkpoint, and that is not an error"
        );
        let cp = sample();
        j.checkpoint(&cp, b"fake-state-bytes").expect("write");
        let (back, state) = Journal::read_checkpoint(&dir).expect("read").expect("some");
        assert_eq!(back, cp);
        assert_eq!(state, b"fake-state-bytes");
        // No stray temp files left behind.
        assert!(!dir.join("checkpoint.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

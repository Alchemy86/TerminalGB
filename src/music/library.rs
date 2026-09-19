//! The shelf: one [`Album`] per cartridge, and the background worker that fills
//! it in.
//!
//! An album is built by *listening to the cartridge*: one headless boot yields
//! both the candidate cover frames and a verdict on whether anything was heard.
//! That boot is a few tenths of a second, which is far too long to do on the
//! render thread — the audio queue's cushion is about 41 ms — so the shelf is
//! filled from a worker thread and the UI shows what it has.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use crate::music::capture::{self, Level};
use crate::music::cover::{self, Cover};
use crate::screen::romlib::RomEntry;

/// Where an album's audio comes from — and, just as importantly, what the shelf
/// is allowed to *claim* about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackSource {
    /// Boot the cartridge and record what it plays. Works for every ROM, needs
    /// nothing game-specific, and gets you the title theme. **This is the only
    /// source implemented today**, and the shelf says "title theme", never
    /// "soundtrack".
    Boot,
    /// Drive the game's own sound test. Many cartridges have one, sometimes
    /// behind a button code on the title screen. It is a real source of whole
    /// soundtracks — but reaching it needs per-game knowledge (which screen,
    /// which buttons, how many tracks), which is exactly what a **plugin** is
    /// for rather than a table baked into the emulator. Named, not implemented.
    SoundTest,
    /// Call the game's sound driver directly to enumerate every track. The most
    /// powerful option and by far the most work: a driver has to be reverse
    /// engineered per title. Recorded here as where the format could go; **do
    /// not attempt it inside this module.**
    Driver,
}

impl TrackSource {
    /// The promise this source is allowed to make, in the words the shelf uses.
    pub fn label(self) -> &'static str {
        match self {
            TrackSource::Boot => "title theme",
            TrackSource::SoundTest => "sound test",
            TrackSource::Driver => "full soundtrack",
        }
    }

    /// The longer form, for the album panel. Says what was actually done.
    pub fn describe(self) -> &'static str {
        match self {
            TrackSource::Boot => "what the cartridge plays when it boots",
            TrackSource::SoundTest => "driven through the game's own sound test",
            TrackSource::Driver => "enumerated from the game's sound driver",
        }
    }
}

/// What the listen found. Kept as three states rather than a bool because
/// "silent" and "the sound chip never ran" look identical from the outside and
/// mean opposite things — one is a property of the cartridge, the other is a
/// bug in us.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioVerdict {
    /// Sound was heard.
    Audible,
    /// The APU ran and the cartridge programmed nothing audible in the window
    /// we listened to.
    Silent,
    /// The sound chip never ran. A fault, and the shelf says so.
    NoApu,
    /// The cartridge could not be booted at all (bad header, unsupported
    /// mapper). Carries the reason.
    Failed,
}

impl AudioVerdict {
    pub fn label(self) -> &'static str {
        match self {
            AudioVerdict::Audible => "music",
            AudioVerdict::Silent => "no sound heard",
            AudioVerdict::NoApu => "sound chip did not run",
            AudioVerdict::Failed => "will not boot",
        }
    }
}

/// One cartridge on the shelf.
#[derive(Clone)]
pub struct Album {
    pub entry: RomEntry,
    /// Candidate covers, best first. Empty when the game drew nothing we could
    /// use — the shelf then shows a plain card with the title, which is honest.
    pub covers: Vec<Cover>,
    /// Which candidate is on show.
    pub cover_idx: usize,
    pub source: TrackSource,
    pub verdict: AudioVerdict,
    /// Level summary of the probe, for the album panel.
    pub level: Level,
    /// Why the album could not be built, when `verdict` is
    /// [`AudioVerdict::Failed`].
    pub error: Option<String>,
    /// Wall-clock cost of the probe, milliseconds — shown in the panel because a
    /// shelf that takes a while to fill should say what it is doing.
    pub probe_ms: u32,
    /// Whether the worker has listened to this cartridge yet. False on a
    /// placeholder — and the distinction matters, because starting an album
    /// before its probe lands means starting it at the publisher logo rather
    /// than at its title screen.
    pub probed: bool,
}

impl Album {
    /// Where this album's music starts, and the machine as it stood there.
    ///
    /// It is the *cover's own scene*: whatever the picture is showing, the
    /// sound that goes with it started when that picture came up. Pokémon Gold
    /// plays a minute of intro before its title screen — without this, the
    /// shelf would show the title and play the intro, and a "sixty second"
    /// export would be sixty seconds of the wrong music.
    pub fn start(&self) -> Start {
        match self.cover() {
            Some(c) => Start {
                frame: c.scene_start,
                state: c.state.clone(),
            },
            None => Start::default(),
        }
    }
}

impl Album {
    /// A placeholder shown while the worker is still listening to this
    /// cartridge. The title and mapper come from the header, which is free.
    pub fn pending(entry: RomEntry) -> Album {
        Album {
            entry,
            covers: Vec::new(),
            cover_idx: 0,
            source: TrackSource::Boot,
            verdict: AudioVerdict::Silent,
            level: Level {
                peak: 0.0,
                rms: 0.0,
                silent_frac: 1.0,
            },
            error: None,
            probe_ms: 0,
            probed: false,
        }
    }

    pub fn cover(&self) -> Option<&Cover> {
        self.covers.get(self.cover_idx)
    }

    /// Show the next candidate cover. No-op with fewer than two.
    pub fn cycle_cover(&mut self) {
        if self.covers.len() > 1 {
            self.cover_idx = (self.cover_idx + 1) % self.covers.len();
        }
    }

    /// The one-line claim the shelf makes about this album's audio. Never says
    /// more than was actually done.
    pub fn now_playing_label(&self) -> String {
        match self.verdict {
            AudioVerdict::Audible => {
                format!("{} · {}", self.source.label(), self.source.describe())
            }
            other => other.label().to_string(),
        }
    }
}

/// How thoroughly to listen before putting a cartridge on the shelf.
#[derive(Clone, Copy, Debug)]
pub struct ProbeOptions {
    /// Seconds of boot to run. Long enough to get past a publisher logo and an
    /// attract animation on a slow title.
    pub seconds: f64,
    pub cover: cover::Options,
}

impl Default for ProbeOptions {
    fn default() -> ProbeOptions {
        ProbeOptions {
            // Measured against real cartridges, and this is why it is not
            // shorter: Pokémon Blue reaches its title screen at ~25 s and
            // Pokémon Gold/Silver at ~64 s, after a long intro. At 26 s Gold
            // offered the sea behind its intro as its cover, which is a
            // perfectly good picture of the wrong thing. Costs ~2 s of CPU per
            // cartridge, on a worker thread, while the shelf is already usable.
            seconds: 75.0,
            cover: cover::Options::default(),
        }
    }
}

/// Build one album by booting the cartridge once.
pub fn probe(entry: RomEntry, rom: &[u8], opts: ProbeOptions) -> Album {
    let started = std::time::Instant::now();
    let mut picker = cover::Picker::new(opts.cover);
    let result = capture::record_with(
        rom,
        capture::Options {
            frames: capture::frames_for(opts.seconds),
            ..capture::Options::default()
        },
        |view| picker.observe(&view),
    );
    let probe_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;

    match result {
        Err(e) => Album {
            verdict: AudioVerdict::Failed,
            error: Some(e),
            probe_ms,
            probed: true,
            ..Album::pending(entry)
        },
        Ok(rec) => {
            let level = capture::level(&rec.samples);
            let verdict = if !rec.apu_ran {
                AudioVerdict::NoApu
            } else if capture::is_audible(level) {
                AudioVerdict::Audible
            } else {
                AudioVerdict::Silent
            };
            Album {
                covers: picker.finish(),
                verdict,
                level,
                probe_ms,
                probed: true,
                ..Album::pending(entry)
            }
        }
    }
}

/// The shelf, and the worker filling it.
///
/// Every cartridge is on the shelf **immediately** with its header title, and
/// gains its cover and verdict when the worker reaches it. A library that takes
/// four seconds to appear is a library nobody browses.
pub struct Library {
    albums: Vec<Album>,
    rx: Option<Receiver<(usize, Album)>>,
    done: usize,
}

impl Library {
    /// Scan `dir` and start listening to everything in it.
    pub fn scan(dir: &Path, opts: ProbeOptions) -> Library {
        Library::from_entries(crate::screen::romlib::scan_dir(dir), opts)
    }

    pub fn from_entries(entries: Vec<RomEntry>, opts: ProbeOptions) -> Library {
        let albums: Vec<Album> = entries.into_iter().map(Album::pending).collect();
        if albums.is_empty() {
            return Library {
                albums,
                rx: None,
                done: 0,
            };
        }
        let work: Vec<(usize, RomEntry)> = albums
            .iter()
            .enumerate()
            .map(|(i, a)| (i, a.entry.clone()))
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || probe_all(work, opts, tx));
        Library {
            albums,
            rx: Some(rx),
            done: 0,
        }
    }

    /// A library built from bytes already in hand, with no worker. Used by the
    /// demo shelf and the tests, which have no ROM directory.
    pub fn from_roms(roms: Vec<(RomEntry, Vec<u8>)>, opts: ProbeOptions) -> Library {
        let albums = roms
            .into_iter()
            .map(|(e, data)| probe(e, &data, opts))
            .collect::<Vec<_>>();
        let done = albums.len();
        Library {
            albums,
            rx: None,
            done,
        }
    }

    /// Take whatever the worker has finished since last time. Never blocks.
    /// Returns `true` if anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = self.rx.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok((i, album)) => {
                    if let Some(slot) = self.albums.get_mut(i) {
                        *slot = album;
                    }
                    self.done += 1;
                    changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }

    pub fn albums(&self) -> &[Album] {
        &self.albums
    }
    pub fn albums_mut(&mut self) -> &mut [Album] {
        &mut self.albums
    }
    pub fn len(&self) -> usize {
        self.albums.len()
    }
    pub fn is_empty(&self) -> bool {
        self.albums.is_empty()
    }
    /// Albums whose probe has finished, out of the total.
    pub fn progress(&self) -> (usize, usize) {
        (self.done.min(self.albums.len()), self.albums.len())
    }
    pub fn ready(&self) -> bool {
        self.done >= self.albums.len()
    }
    pub fn get(&self, i: usize) -> Option<&Album> {
        self.albums.get(i)
    }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut Album> {
        self.albums.get_mut(i)
    }
}

fn probe_all(
    work: Vec<(usize, RomEntry)>,
    opts: ProbeOptions,
    tx: Sender<(usize, Album)>,
) {
    for (i, entry) in work {
        let album = match std::fs::read(&entry.path) {
            Ok(data) => probe(entry, &data, opts),
            Err(e) => Album {
                verdict: AudioVerdict::Failed,
                error: Some(e.to_string()),
                probed: true,
                ..Album::pending(entry)
            },
        };
        // A closed channel means the shelf was left; stop working for nobody.
        if tx.send((i, album)).is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// How long an exported WAV is, by default.
pub const DEFAULT_EXPORT_SECONDS: f64 = 60.0;

/// What one export produced.
pub struct Export {
    pub path: PathBuf,
    pub seconds: f64,
    pub bytes: usize,
    pub verdict: AudioVerdict,
}

/// Where in the boot an album's music starts, and the machine as it stood
/// there — both taken from the scene the cover came from.
///
/// See [`Album::start`]. Playing and exporting both begin here, which is what
/// makes the picture and the sound agree.
#[derive(Clone, Default)]
pub struct Start {
    pub frame: u32,
    pub state: Option<Vec<u8>>,
}

impl Start {
    pub fn seconds(&self) -> f64 {
        self.frame as f64 / capture::FPS
    }
}

/// Render `seconds` of a cartridge's own audio and write it as a WAV.
///
/// This re-boots the cartridge headlessly rather than tapping the live stream,
/// for three reasons worth keeping: the take starts at the beginning instead of
/// wherever the listener happened to be, it is deterministic (the same ROM and
/// the same length give byte-identical output), and it does not perturb
/// playback — a 60-second export renders in about a second of CPU while the
/// shelf keeps playing.
///
/// The encoder is `terminalgb_chiptune::wav`, which the chiptune renderer also
/// uses. There is deliberately only one WAV writer in this repository.
pub fn export_wav(
    rom: &[u8],
    dest: &Path,
    seconds: f64,
    start: Option<&[u8]>,
) -> Result<Export, String> {
    let mut rec = capture::record_from(
        rom,
        start,
        capture::Options {
            frames: capture::frames_for(seconds),
            ..capture::Options::default()
        },
        |_| {},
    )?;
    let verdict = if !rec.apu_ran {
        AudioVerdict::NoApu
    } else if capture::is_audible(capture::level(&rec.samples)) {
        AudioVerdict::Audible
    } else {
        AudioVerdict::Silent
    };
    capture::trim_and_fade(&mut rec, seconds);
    let bytes = terminalgb_chiptune::wav::encode(&rec.samples, rec.rate);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(dest, &bytes).map_err(|e| e.to_string())?;
    Ok(Export {
        path: dest.to_path_buf(),
        seconds: rec.seconds(),
        bytes: bytes.len(),
        verdict,
    })
}

/// A file name for an exported track: the cartridge title, made safe, plus the
/// take length. Never the ROM's own path — an export belongs in the user's music
/// folder, not beside their cartridge.
pub fn export_name(title: &str, seconds: f64) -> String {
    let mut safe = String::new();
    let mut last_dash = false;
    for c in title.trim().chars() {
        if c.is_ascii_alphanumeric() {
            safe.push(c);
            last_dash = false;
        } else if !last_dash && !safe.is_empty() {
            safe.push('-');
            last_dash = true;
        }
    }
    while safe.ends_with('-') {
        safe.pop();
    }
    if safe.is_empty() {
        safe.push_str("gameboy");
    }
    format!("{safe}-title-theme-{}s.wav", seconds.round() as u64)
}

/// Never overwrite: `NAME.wav`, then `NAME-2.wav`, ...
pub fn unique_export_path(dir: &Path, name: &str) -> PathBuf {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), e.to_string()),
        None => (name.to_string(), "wav".to_string()),
    };
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() && n < 1000 {
        path = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::testrom;
    use std::path::Path;

    fn entry(name: &str, rom: &[u8]) -> RomEntry {
        crate::screen::romlib::entry_from_bytes(Path::new(name), rom)
    }

    fn quick() -> ProbeOptions {
        ProbeOptions {
            seconds: 1.5,
            cover: cover::Options {
                sample_every: 4,
                warmup: 4,
                keep: 4,
                keep_states: true,
            },
        }
    }

    #[test]
    fn a_cartridge_that_plays_and_draws_becomes_a_full_album() {
        let rom = testrom::tone_rom();
        let a = probe(entry("/x/tone.gb", &rom), &rom, quick());
        assert_eq!(a.entry.title, "GB TONE 440");
        assert_eq!(a.verdict, AudioVerdict::Audible, "level {:?}", a.level);
        assert!(!a.covers.is_empty(), "it draws a chequerboard");
        assert_eq!(a.source, TrackSource::Boot);
        assert!(a.now_playing_label().contains("title theme"));
    }

    /// The whole point of the three-state verdict.
    #[test]
    fn a_silent_cartridge_says_no_sound_heard_not_nothing() {
        let rom = testrom::silent_rom();
        let a = probe(entry("/x/quiet.gb", &rom), &rom, quick());
        assert_eq!(a.verdict, AudioVerdict::Silent);
        assert_eq!(a.now_playing_label(), "no sound heard");
        assert!(!a.covers.is_empty(), "it still draws a picture");
    }

    #[test]
    fn a_cartridge_that_draws_nothing_offers_no_cover() {
        let rom = testrom::blank_rom();
        let a = probe(entry("/x/blank.gb", &rom), &rom, quick());
        assert!(a.covers.is_empty(), "a blank screen is not artwork");
        assert!(a.cover().is_none());
    }

    #[test]
    fn unreadable_bytes_are_an_album_that_says_why() {
        let rom = vec![0u8; 64];
        let a = probe(entry("/x/junk.gb", &rom), &rom, quick());
        assert_eq!(a.verdict, AudioVerdict::Failed);
        assert!(a.error.is_some());
        assert_eq!(a.now_playing_label(), "will not boot");
    }

    /// **The shelf must never claim more than it did.** Every wording path is
    /// checked because this is the promise the maintainer asked us to keep.
    #[test]
    fn the_shelf_never_claims_a_soundtrack_it_did_not_get() {
        let rom = testrom::tone_rom();
        let a = probe(entry("/x/tone.gb", &rom), &rom, quick());
        let said = a.now_playing_label().to_ascii_lowercase();
        assert!(
            !said.contains("soundtrack") && !said.contains("full"),
            "a boot capture is a title theme, not a soundtrack: {said:?}"
        );
        assert_eq!(TrackSource::Boot.label(), "title theme");
    }

    #[test]
    fn cycling_covers_wraps_and_is_a_no_op_with_one() {
        let mut a = Album::pending(entry("/x/a.gb", &[0u8; 0x150]));
        a.cycle_cover();
        assert_eq!(a.cover_idx, 0);
        a.covers = vec![
            Cover {
                frame: 1,
                score: 3,
                ..Cover::default()
            },
            Cover {
                frame: 2,
                score: 2,
                ..Cover::default()
            },
        ];
        a.cycle_cover();
        assert_eq!(a.cover_idx, 1);
        a.cycle_cover();
        assert_eq!(a.cover_idx, 0, "wraps");
    }

    #[test]
    fn export_writes_a_playable_wav_from_a_real_boot() {
        let dir =
            std::env::temp_dir().join(format!("gb-music-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let rom = testrom::tone_rom();
        let path = dir.join("tone.wav");
        let ex = export_wav(&rom, &path, 0.5, None).expect("export");
        assert_eq!(ex.verdict, AudioVerdict::Audible);
        let bytes = std::fs::read(&path).expect("written");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        // 16-bit stereo at the capture rate, half a second of it.
        let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        assert_eq!(rate, capture::RATE);
        let data_len =
            u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
        assert_eq!(data_len, bytes.len() - 44);
        let expect = (capture::RATE as f64 * 0.5) as usize * 4;
        assert!(
            data_len.abs_diff(expect) < 4096,
            "expected ~{expect} bytes of PCM, got {data_len}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Rendering the same cartridge twice must give the same file — that is what
    /// makes an export something a person can re-make rather than a one-off.
    #[test]
    fn an_export_is_deterministic() {
        let rom = testrom::tone_rom();
        let dir =
            std::env::temp_dir().join(format!("gb-music-det-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.wav");
        let b = dir.join("b.wav");
        export_wav(&rom, &a, 0.3, None).unwrap();
        export_wav(&rom, &b, 0.3, None).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn export_names_are_safe_and_say_what_they_are() {
        assert_eq!(
            export_name("POKEMON BLUE", 60.0),
            "POKEMON-BLUE-title-theme-60s.wav"
        );
        assert_eq!(
            export_name("  ../../etc  ", 30.0),
            "etc-title-theme-30s.wav"
        );
        assert_eq!(export_name("", 30.0), "gameboy-title-theme-30s.wav");
        assert!(!export_name("a/b\\c", 30.0).contains('/'));
    }

    #[test]
    fn a_second_export_never_overwrites_the_first() {
        let dir =
            std::env::temp_dir().join(format!("gb-music-uniq-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p1 = unique_export_path(&dir, "x.wav");
        std::fs::write(&p1, b"1").unwrap();
        let p2 = unique_export_path(&dir, "x.wav");
        assert_ne!(p1, p2);
        assert!(p2.to_string_lossy().ends_with("x-2.wav"));
        let _ = std::fs::remove_file(&p1);
    }

    #[test]
    fn a_library_built_in_process_is_ready_immediately() {
        let tone = testrom::tone_rom();
        let quiet = testrom::silent_rom();
        let lib = Library::from_roms(
            vec![
                (entry("/x/tone.gb", &tone), tone.clone()),
                (entry("/x/quiet.gb", &quiet), quiet.clone()),
            ],
            quick(),
        );
        assert_eq!(lib.len(), 2);
        assert!(lib.ready());
        assert_eq!(lib.progress(), (2, 2));
        assert_eq!(lib.get(0).unwrap().verdict, AudioVerdict::Audible);
        assert_eq!(lib.get(1).unwrap().verdict, AudioVerdict::Silent);
    }

    /// A shelf with no ROMs is empty, not an error, and it does not start a
    /// worker that would never finish.
    #[test]
    fn an_empty_directory_is_an_empty_shelf() {
        let lib = Library::from_entries(Vec::new(), quick());
        assert!(lib.is_empty());
        assert!(lib.ready());
    }

    /// The worker fills the shelf without blocking the caller: every album is
    /// present with its title from the first moment.
    #[test]
    fn a_scanned_library_shows_titles_before_it_has_covers() {
        let dir =
            std::env::temp_dir().join(format!("gb-music-scan-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("tone.gb"), testrom::tone_rom()).unwrap();
        std::fs::write(dir.join("quiet.gb"), testrom::silent_rom()).unwrap();
        let mut lib = Library::scan(&dir, quick());
        assert_eq!(lib.len(), 2, "both titles are on the shelf at once");
        assert!(lib.albums().iter().all(|a| !a.entry.title.is_empty()));
        // Drain the worker.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !lib.ready() && std::time::Instant::now() < deadline {
            lib.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(lib.ready(), "worker finished");
        assert!(lib.albums().iter().any(|a| !a.covers.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Music mode must never touch a save file. A probe boots the cartridge, and
    /// a cartridge with battery-backed RAM would write one beside the ROM if the
    /// machine were given its path.
    #[test]
    fn probing_a_battery_cartridge_writes_nothing_beside_it() {
        let dir =
            std::env::temp_dir().join(format!("gb-music-sav-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // A cartridge that *writes* to battery-backed RAM. One that merely
        // declares it proves nothing: the battery layer deliberately writes
        // nothing for a session that never touched cartridge RAM.
        let rom = testrom::battery_rom();
        let path = dir.join("battery.gb");
        std::fs::write(&path, &rom).unwrap();
        let e = crate::screen::romlib::entry_from_path(&path).unwrap();
        let _ = probe(e, &rom, quick());
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|f| f.ok())
            .map(|f| f.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "battery.gb")
            .collect();
        assert!(
            leftovers.is_empty(),
            "music mode must not write save files: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

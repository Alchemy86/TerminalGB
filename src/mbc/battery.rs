//! Battery-backed cartridge RAM persistence.
//!
//! On real hardware a coin cell keeps cartridge RAM alive the instant the game
//! writes it. This module is the emulator's equivalent, and it exists because
//! the original code only wrote the save in `impl Drop for MBCx` — which is a
//! promise the frontends cannot keep:
//!
//! * the desktop frontend's `glutin`/`winit` `event_loop.run()` **never
//!   returns** (winit 0.26 calls `process::exit(0)` on `ControlFlow::Exit`), so
//!   the `Gameboy` moved into the closure is never dropped;
//! * `[profile.release] panic = 'abort'` means a panic skips destructors;
//! * closing the terminal window, SIGTERM, or a `kill` skip them too.
//!
//! Any of those loses everything the player saved. Worse, the *next* launch
//! then finds no save file, boots a fresh game, and on a clean exit writes its
//! pristine all-zero RAM — so a single unclean exit turns into a permanently
//! empty save. See `docs/components/mbc.md`.
//!
//! The fix here: cartridge RAM is written out **while the emulator runs**,
//! shortly after the game stops touching it ([`Battery::tick`]), and every
//! write is atomic (temp file + `rename`) so an interrupted write can never
//! truncate an existing save.
//!
//! ## File formats
//!
//! * `<rom>.sav` — **raw cartridge RAM, no header**, exactly `ramsize` bytes.
//!   This is what every other emulator uses, so saves are interchangeable in
//!   both directions.
//! * `<rom>.rtc` — MBC3 real-time-clock sidecar, 48 bytes little-endian, the
//!   layout VBA-M/mGBA append to their `.sav`: 5 × `u32` current registers
//!   (s, m, h, day-low, day-high), 5 × `u32` latched registers, then a `u64`
//!   UNIX timestamp of when it was written.
//! * `<rom>.gbsave` — this emulator's **legacy** format (8-byte big-endian RTC
//!   anchor followed by the RAM). Still *read* when no `.sav` exists, never
//!   written again; the old file is left in place untouched as a backup.
//!
//! On load a `.sav` of `ramsize + 48` bytes is also accepted — that is the
//! VBA-M/mGBA convention of appending the RTC struct to the save — so their
//! files load here directly.

use std::io::prelude::*;
use std::{fs, io, path};

use crate::mbc::StrResult;

/// How long the cartridge RAM must go untouched before it is written out.
/// One second at 59.7 fps: long enough that a game's multi-frame save burst
/// becomes a single disk write, short enough that a `kill -9` a moment later
/// still has the save on disk.
const SETTLE_FRAMES: u32 = 60;

/// Hard ceiling, in frames, on how long dirty RAM may go unwritten. Covers the
/// rare game that dribbles into SRAM continuously and so never "settles".
const MAX_DIRTY_FRAMES: u32 = 600;

/// The MBC3 real-time clock, in the neutral shape the sidecar file stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcState {
    /// Live registers: seconds, minutes, hours, day-low, day-high/flags.
    pub regs: [u8; 5],
    /// The latched copy the game reads through 0xA000-0xBFFF.
    pub latched: [u8; 5],
    /// UNIX seconds at which this snapshot was taken, so the clock can keep
    /// running while the emulator is closed (as the cartridge battery does).
    pub timestamp: u64,
}

/// What [`Battery::load`] recovered from disk.
pub struct Loaded {
    /// Cartridge RAM, always exactly `ramsize` bytes (padded/truncated).
    pub ram: Vec<u8>,
    /// RTC from a `.rtc` sidecar or an appended VBA-M/mGBA block, if any.
    pub rtc: Option<RtcState>,
    /// Legacy `.gbsave` RTC anchor (UNIX seconds), when that is all we have.
    pub legacy_rtc_zero: Option<u64>,
    /// True when nothing was found — the cartridge starts blank.
    pub fresh: bool,
}

/// Persistence for one cartridge's battery-backed RAM.
pub struct Battery {
    sav: Option<path::PathBuf>,
    rtc: Option<path::PathBuf>,
    legacy: Option<path::PathBuf>,
    ramsize: usize,
    /// RAM has changed since the last successful write.
    dirty: bool,
    /// Frames since the most recent cartridge-RAM write.
    quiet: u32,
    /// Frames since the RAM first became dirty.
    age: u32,
    /// Set when reading an existing save failed. Refuse to write after that —
    /// a save we could not read is a save we must not overwrite.
    poisoned: bool,
}

impl Battery {
    /// A battery for `rom` with `ramsize` bytes of cartridge RAM. `ramsize == 0`
    /// (a cart claiming battery-backed RAM but declaring none) gets no files.
    pub fn new(rom: &path::Path, ramsize: usize) -> Battery {
        if ramsize == 0 {
            return Battery::none();
        }
        Battery {
            sav: Some(rom.with_extension("sav")),
            rtc: Some(rom.with_extension("rtc")),
            legacy: Some(rom.with_extension("gbsave")),
            ramsize,
            dirty: false,
            quiet: 0,
            age: 0,
            poisoned: false,
        }
    }

    /// A battery that persists nothing (no cartridge battery, or no file path —
    /// the web and FFI builds).
    pub fn none() -> Battery {
        Battery {
            sav: None,
            rtc: None,
            legacy: None,
            ramsize: 0,
            dirty: false,
            quiet: 0,
            age: 0,
            poisoned: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.sav.is_some()
    }

    /// Cartridge RAM (or the clock) has changed since the last write. Callers
    /// use this to skip work that only matters when a write is pending — MBC3
    /// re-derives its RTC registers from the host clock, which is pointless on
    /// a frame where nothing is going to be saved.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Read the save, preferring `<rom>.sav` and falling back to the legacy
    /// `<rom>.gbsave`. A missing file is not an error (a brand-new cartridge);
    /// anything else is, because silently starting fresh would let the next
    /// write destroy a save we simply failed to read.
    pub fn load(&mut self) -> StrResult<Loaded> {
        let blank = |b: &Battery| Loaded {
            ram: vec![0; b.ramsize],
            rtc: None,
            legacy_rtc_zero: None,
            fresh: true,
        };
        let Some(ref sav) = self.sav else {
            return Ok(blank(self));
        };

        match read_file(sav) {
            Ok(Some(bytes)) => {
                // VBA-M/mGBA append the 48-byte RTC block to the .sav; accept it.
                let (ram_bytes, trailing) = if bytes.len() == self.ramsize + RTC_BYTES {
                    bytes.split_at(self.ramsize)
                } else {
                    (&bytes[..], &[][..])
                };
                let mut rtc = decode_rtc(trailing);
                if rtc.is_none() {
                    rtc = self.read_rtc_sidecar()?;
                }
                Ok(Loaded {
                    ram: self.fit(ram_bytes),
                    rtc,
                    legacy_rtc_zero: None,
                    fresh: false,
                })
            }
            Ok(None) => self.load_legacy(),
            Err(e) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }

    /// Legacy `<rom>.gbsave`: 8 bytes of big-endian RTC anchor, then the RAM.
    fn load_legacy(&mut self) -> StrResult<Loaded> {
        let Some(ref legacy) = self.legacy else {
            return Ok(Loaded {
                ram: vec![0; self.ramsize],
                rtc: None,
                legacy_rtc_zero: None,
                fresh: true,
            });
        };
        match read_file(legacy) {
            Ok(None) => Ok(Loaded {
                ram: vec![0; self.ramsize],
                rtc: None,
                legacy_rtc_zero: None,
                fresh: true,
            }),
            Ok(Some(bytes)) => {
                if bytes.len() < 8 {
                    self.poisoned = true;
                    return Err("legacy .gbsave file is truncated");
                }
                let mut anchor = [0u8; 8];
                anchor.copy_from_slice(&bytes[..8]);
                Ok(Loaded {
                    ram: self.fit(&bytes[8..]),
                    rtc: None,
                    legacy_rtc_zero: Some(u64::from_be_bytes(anchor)),
                    // Migrated content must be written out in the new format.
                    fresh: false,
                })
            }
            Err(e) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }

    fn read_rtc_sidecar(&mut self) -> StrResult<Option<RtcState>> {
        let Some(ref path) = self.rtc else {
            return Ok(None);
        };
        match read_file(path) {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => Ok(decode_rtc(&bytes)),
            Err(e) => {
                self.poisoned = true;
                Err(e)
            }
        }
    }

    /// Normalise a save to exactly `ramsize` bytes. Short files are zero-padded
    /// and long ones truncated rather than rejected: emulators disagree about
    /// trailing padding, and a wrong-length `Vec` would make the MBCs index out
    /// of bounds (a hard abort under `panic = 'abort'`).
    fn fit(&self, bytes: &[u8]) -> Vec<u8> {
        let mut ram = vec![0u8; self.ramsize];
        let n = bytes.len().min(self.ramsize);
        ram[..n].copy_from_slice(&bytes[..n]);
        ram
    }

    /// The game wrote to cartridge RAM. Restarts the settle timer.
    pub fn touch(&mut self) {
        if !self.dirty {
            self.age = 0;
        }
        self.dirty = true;
        self.quiet = 0;
    }

    /// Force a write on the next [`Battery::tick`] even if nothing has been
    /// written since — used after a save-state load replaces the RAM wholesale,
    /// and to migrate a legacy `.gbsave` to `.sav` at startup.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.quiet = SETTLE_FRAMES;
    }

    /// Call once per emulated frame. Writes the save out once the game has left
    /// cartridge RAM alone for a moment — this, not `Drop`, is what makes a save
    /// survive a crash or a closed window.
    pub fn tick(&mut self, ram: &[u8], rtc: Option<RtcState>) {
        if !self.dirty {
            return;
        }
        self.quiet = self.quiet.saturating_add(1);
        self.age = self.age.saturating_add(1);
        if self.quiet >= SETTLE_FRAMES || self.age >= MAX_DIRTY_FRAMES {
            self.flush(ram, rtc);
        }
    }

    /// Write the save now, if there is anything to write. Safe to call often.
    pub fn flush(&mut self, ram: &[u8], rtc: Option<RtcState>) {
        if !self.dirty || self.poisoned {
            return;
        }
        let Some(ref sav) = self.sav else {
            self.dirty = false;
            return;
        };
        // Leave `dirty` set if the write fails so the next tick retries.
        if write_atomic(sav, ram).is_err() {
            return;
        }
        if let (Some(path), Some(state)) = (self.rtc.as_ref(), rtc) {
            let _ = write_atomic(path, &encode_rtc(&state));
        }
        self.dirty = false;
        self.quiet = 0;
        self.age = 0;
    }
}

fn read_file(path: &path::Path) -> StrResult<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ref e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("could not read the existing save file"),
    }
}

/// Write `bytes` to `path` without ever leaving a half-written file there: a
/// sibling temp file is written and fsynced, then `rename`d over the target
/// (atomic on POSIX and on Windows' `MoveFileEx`-backed `fs::rename`). A
/// truncated save is worse than no save at all.
fn write_atomic(path: &path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = path::PathBuf::from(tmp);

    // Scoped so the handle is closed before the rename.
    let written = {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes).and_then(|_| f.sync_all())
    };
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // Best-effort: persist the directory entry too. Not portable (opening a
    // directory fails on Windows), so failures are ignored.
    if let Some(dir) = path.parent() {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Size of the VBA-M/mGBA RTC block: 10 x u32 + u64, little-endian.
pub const RTC_BYTES: usize = 48;

fn encode_rtc(state: &RtcState) -> Vec<u8> {
    let mut out = Vec::with_capacity(RTC_BYTES);
    for v in state.regs.iter().chain(state.latched.iter()) {
        out.extend_from_slice(&(*v as u32).to_le_bytes());
    }
    out.extend_from_slice(&state.timestamp.to_le_bytes());
    out
}

fn decode_rtc(bytes: &[u8]) -> Option<RtcState> {
    if bytes.len() < RTC_BYTES {
        return None;
    }
    let u32_at = |i: usize| {
        let mut b = [0u8; 4];
        b.copy_from_slice(&bytes[i * 4..i * 4 + 4]);
        u32::from_le_bytes(b) as u8
    };
    let mut regs = [0u8; 5];
    let mut latched = [0u8; 5];
    for i in 0..5 {
        regs[i] = u32_at(i);
        latched[i] = u32_at(5 + i);
    }
    let mut ts = [0u8; 8];
    ts.copy_from_slice(&bytes[40..48]);
    Some(RtcState {
        regs,
        latched,
        timestamp: u64::from_le_bytes(ts),
    })
}

/// Seconds since the UNIX epoch, or 0 if the clock is before it.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gb-battery-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rtc_block_round_trips() {
        let s = RtcState {
            regs: [1, 2, 3, 4, 0x81],
            latched: [5, 6, 7, 8, 0x40],
            timestamp: 1_700_000_000,
        };
        let bytes = encode_rtc(&s);
        assert_eq!(bytes.len(), RTC_BYTES);
        assert_eq!(decode_rtc(&bytes), Some(s));
    }

    #[test]
    fn writes_raw_sav_with_no_header() {
        let dir = tmpdir("raw");
        let rom = dir.join("game.gb");
        let mut b = Battery::new(&rom, 0x2000);
        let ram = vec![0xABu8; 0x2000];
        b.touch();
        b.flush(&ram, None);
        let bytes = fs::read(dir.join("game.sav")).unwrap();
        // Byte-for-byte the cartridge RAM: what every other emulator expects.
        assert_eq!(bytes, ram);
    }

    #[test]
    fn reads_a_foreign_sav_with_appended_rtc() {
        let dir = tmpdir("foreign");
        let rom = dir.join("game.gb");
        let mut blob = vec![0x11u8; 0x2000];
        blob.extend_from_slice(&encode_rtc(&RtcState {
            regs: [9, 8, 7, 6, 1],
            latched: [9, 8, 7, 6, 1],
            timestamp: 42,
        }));
        fs::write(dir.join("game.sav"), &blob).unwrap();

        let loaded = Battery::new(&rom, 0x2000).load().unwrap();
        assert_eq!(loaded.ram, vec![0x11u8; 0x2000]);
        assert_eq!(loaded.rtc.unwrap().timestamp, 42);
    }

    #[test]
    fn migrates_legacy_gbsave_and_leaves_it_alone() {
        let dir = tmpdir("legacy");
        let rom = dir.join("game.gb");
        let mut legacy = 1234u64.to_be_bytes().to_vec();
        legacy.extend_from_slice(&vec![0x5Au8; 0x2000]);
        fs::write(dir.join("game.gbsave"), &legacy).unwrap();

        let mut b = Battery::new(&rom, 0x2000);
        let loaded = b.load().unwrap();
        assert_eq!(loaded.ram, vec![0x5Au8; 0x2000]);
        assert_eq!(loaded.legacy_rtc_zero, Some(1234));
        assert!(!loaded.fresh);

        b.mark_dirty();
        b.flush(&loaded.ram, None);
        assert_eq!(fs::read(dir.join("game.sav")).unwrap(), loaded.ram);
        // The old file is a backup now: still there, still original.
        assert_eq!(fs::read(dir.join("game.gbsave")).unwrap(), legacy);
    }

    #[test]
    fn short_and_long_saves_are_normalised_not_rejected() {
        let dir = tmpdir("fit");
        let rom = dir.join("game.gb");
        fs::write(dir.join("game.sav"), vec![7u8; 16]).unwrap();
        let loaded = Battery::new(&rom, 0x2000).load().unwrap();
        assert_eq!(loaded.ram.len(), 0x2000);
        assert_eq!(&loaded.ram[..16], &[7u8; 16]);
        assert_eq!(loaded.ram[16], 0);

        fs::write(dir.join("game.sav"), vec![3u8; 0x4000]).unwrap();
        let loaded = Battery::new(&rom, 0x2000).load().unwrap();
        assert_eq!(loaded.ram, vec![3u8; 0x2000]);
    }

    #[test]
    fn a_clean_boot_never_creates_a_save_file() {
        let dir = tmpdir("noboot");
        let rom = dir.join("game.gb");
        let mut b = Battery::new(&rom, 0x2000);
        let _ = b.load().unwrap();
        for _ in 0..1000 {
            b.tick(&vec![0u8; 0x2000], None);
        }
        b.flush(&vec![0u8; 0x2000], None);
        // The old Drop-based code wrote an all-zero file here, which is exactly
        // how a good save got replaced by an empty one.
        assert!(!dir.join("game.sav").exists());
    }

    #[test]
    fn autosave_lands_without_any_drop() {
        let dir = tmpdir("autosave");
        let rom = dir.join("game.gb");
        let mut b = Battery::new(&rom, 0x2000);
        let ram = vec![0xC3u8; 0x2000];
        b.touch();
        for _ in 0..SETTLE_FRAMES {
            b.tick(&ram, None);
        }
        assert_eq!(fs::read(dir.join("game.sav")).unwrap(), ram);
    }

    #[test]
    fn a_failed_read_never_overwrites_the_file() {
        let dir = tmpdir("poison");
        let rom = dir.join("game.gb");
        // A legacy file too short to hold its own header: unreadable, but real.
        fs::write(dir.join("game.gbsave"), vec![1u8; 4]).unwrap();
        let mut b = Battery::new(&rom, 0x2000);
        assert!(b.load().is_err());
        b.touch();
        b.flush(&vec![0u8; 0x2000], None);
        assert!(!dir.join("game.sav").exists());
        assert_eq!(fs::read(dir.join("game.gbsave")).unwrap(), vec![1u8; 4]);
    }

    #[test]
    fn atomic_write_leaves_no_temp_behind() {
        let dir = tmpdir("atomic");
        let path = dir.join("x.sav");
        write_atomic(&path, &[1, 2, 3]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), vec![1, 2, 3]);
        assert!(!dir.join("x.sav.tmp").exists());
    }
}

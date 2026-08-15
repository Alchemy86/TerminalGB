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
//! `<rom>.sav` — **raw cartridge RAM, no header, bank 0 first**, exactly
//! `ramsize` bytes. This is what the ecosystem uses: it is byte-for-byte what
//! FlashGBX writes when it dumps a real cartridge (`LK_Device.py` walks the RAM
//! banks in order and appends 0x2000 bytes each), and what every emulator reads.
//!
//! **An MBC3 real-time clock is appended to that same file as a 48-byte
//! footer** — the VBA-M / BGB / FlashGBX convention:
//!
//! ```text
//! offset  size  meaning                                    (all little-endian)
//! 0x00    4     RTC S   seconds 0-59                       u32
//! 0x04    4     RTC M   minutes 0-59                       u32
//! 0x08    4     RTC H   hours   0-23                       u32
//! 0x0C    4     RTC DL  day counter, low 8 bits            u32
//! 0x10    4     RTC DH  bit0 = day bit 8, bit6 = HALT, bit7 = day CARRY
//! 0x14   20     the same five registers again — the latched copy
//! 0x28    8     UNIX timestamp when the file was written   u64
//!         = 0x30 (48) total
//! ```
//!
//! The footer is written **only for cartridges that actually have an RTC**
//! (header types `0x0F` and `0x10`). Type `0x13` is MBC3+RAM+BATTERY with *no*
//! clock, and writing anything extra for it is what made our old saves
//! unreadable elsewhere. Detection on load is `filesize - ramsize`: 48 (or 44,
//! an older variant with a 32-bit timestamp) means a footer is present.
//!
//! The stored timestamp is what makes "the clock kept running while the console
//! was off" work, exactly as the coin cell does on hardware: the registers are
//! anchored at the moment they were saved and the elapsed wall time carries them
//! forward.
//!
//! ### Legacy
//!
//! `<rom>.gbsave` — this emulator's old format (8-byte **big-endian** RTC epoch
//! anchor prepended to the RAM). Nothing else in the world reads it. It is still
//! *read* when no `.sav` exists and migrated on the next write; the old file is
//! left in place untouched as a backup and never written again. `<rom>.srm` (the
//! other common battery-RAM extension) is accepted on load too.

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

/// The MBC3 real-time clock, in the shape the 48-byte save footer stores.
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
    /// RTC decoded from the file's trailing footer, if it had one.
    pub rtc: Option<RtcState>,
    /// Legacy `.gbsave` RTC anchor (UNIX seconds), when that is all we have.
    pub legacy_rtc_zero: Option<u64>,
    /// True when nothing was found — the cartridge starts blank.
    pub fresh: bool,
}

/// Persistence for one cartridge's battery-backed RAM.
pub struct Battery {
    sav: Option<path::PathBuf>,
    /// Other battery-RAM extensions accepted when loading (never written).
    alternates: Vec<path::PathBuf>,
    legacy: Option<path::PathBuf>,
    ramsize: usize,
    /// RAM has changed since the last successful write.
    dirty: bool,
    /// Frames since the most recent cartridge-RAM write.
    quiet: u32,
    /// Frames since the RAM first became dirty.
    age: u32,
    /// Why writing has been refused, if it has. Set when reading an existing
    /// save failed — a save we could not read is a save we must not overwrite —
    /// and by [`Battery::poison`] when the machine has faulted.
    poisoned: Option<String>,
}

impl Battery {
    /// A battery for `rom` with `ramsize` bytes of cartridge RAM. `has_rtc` is
    /// true only for header types `0x0F`/`0x10`, the MBC3 variants that really
    /// carry a clock — it is what decides whether a 48-byte footer is written.
    ///
    /// A cartridge with neither RAM nor a clock gets no files at all.
    pub fn new(rom: &path::Path, ramsize: usize, has_rtc: bool) -> Battery {
        if ramsize == 0 && !has_rtc {
            return Battery::none();
        }
        Battery {
            sav: Some(rom.with_extension("sav")),
            alternates: vec![rom.with_extension("srm")],
            legacy: Some(rom.with_extension("gbsave")),
            ramsize,
            dirty: false,
            quiet: 0,
            age: 0,
            poisoned: None,
        }
    }

    /// A battery that persists nothing (no cartridge battery, or no file path —
    /// the web and FFI builds).
    pub fn none() -> Battery {
        Battery {
            sav: None,
            alternates: Vec::new(),
            legacy: None,
            ramsize: 0,
            dirty: false,
            quiet: 0,
            age: 0,
            poisoned: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.sav.is_some()
    }

    /// **Refuse every future write to the save file.**
    ///
    /// The battery already refused to write after a failed *read*, on the rule
    /// that a save we could not read is a save we must not overwrite. This is
    /// the same rule pointed the other way: once something has established that
    /// the emulated program is no longer executing its own code, the bytes in
    /// cartridge RAM are no longer a save — they are whatever a runaway CPU
    /// happened to store there — and writing them out destroys the good save
    /// still on disk. Keeping the last good file and telling the user is
    /// strictly better than persisting damage.
    ///
    /// Deliberately one-way and cheap: there is no `unpoison`. A machine that
    /// has left the rails cannot argue its way back, and the recovery a user
    /// wants is to restart the emulator, where the untouched `.sav` is waiting.
    ///
    /// Idempotent: the *first* reason is kept, because it is the one that
    /// diagnoses the fault; later ones are consequences.
    pub fn poison(&mut self, reason: &str) {
        if self.poisoned.is_none() {
            self.poisoned = Some(reason.to_string());
        }
    }

    /// Why the save file is being left alone, if it is.
    pub fn poisoned_reason(&self) -> Option<&str> {
        self.poisoned.as_deref()
    }

    /// Cartridge RAM (or the clock) has changed since the last write. Callers
    /// use this to skip work that only matters when a write is pending — MBC3
    /// re-derives its RTC registers from the host clock, which is pointless on
    /// a frame where nothing is going to be saved.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Read the save: `<rom>.sav` first, then the other common battery-RAM
    /// extensions, then the legacy `<rom>.gbsave`. A missing file is not an
    /// error (a brand-new cartridge); anything else is, because silently
    /// starting fresh would let the next write destroy a save we merely failed
    /// to read.
    pub fn load(&mut self) -> StrResult<Loaded> {
        let blank = |b: &Battery| Loaded {
            ram: vec![0; b.ramsize],
            rtc: None,
            legacy_rtc_zero: None,
            fresh: true,
        };
        if self.sav.is_none() {
            return Ok(blank(self));
        }

        let mut candidates: Vec<path::PathBuf> = self.sav.iter().cloned().collect();
        candidates.extend(self.alternates.iter().cloned());
        for path in candidates {
            match read_file(&path) {
                Ok(None) => continue,
                Ok(Some(bytes)) => {
                    // Footer detection is by size, as FlashGBX does it: whatever
                    // the file carries beyond the cartridge's RAM size is the RTC
                    // block. 48 bytes is the standard; 44 is an older variant
                    // whose timestamp is 32-bit.
                    let extra = bytes.len().saturating_sub(self.ramsize);
                    let (ram_bytes, trailing) = if bytes.len() > self.ramsize
                        && (extra == RTC_BYTES || extra == RTC_BYTES_32)
                    {
                        bytes.split_at(self.ramsize)
                    } else {
                        (&bytes[..], &[][..])
                    };
                    return Ok(Loaded {
                        ram: self.fit(ram_bytes),
                        rtc: decode_rtc(trailing),
                        legacy_rtc_zero: None,
                        fresh: false,
                    });
                }
                Err(e) => {
                    self.poison(e);
                    return Err(e);
                }
            }
        }
        self.load_legacy()
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
                    self.poison("legacy .gbsave file is truncated");
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
                self.poison(e);
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
        if !self.dirty || self.poisoned.is_some() {
            return;
        }
        let Some(ref sav) = self.sav else {
            self.dirty = false;
            return;
        };
        // RAM first, then the RTC footer in the same file — that is the layout
        // FlashGBX writes and every emulator expects. No footer for a cartridge
        // without a clock: an unexpected 48 trailing bytes is precisely what
        // made our old saves unreadable elsewhere.
        let mut blob = ram.to_vec();
        if let Some(state) = rtc {
            blob.extend_from_slice(&encode_rtc(&state));
        }
        // Leave `dirty` set if the write fails so the next tick retries.
        if write_atomic(sav, &blob).is_err() {
            return;
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
        // **A `std` that cannot say "no such file" must not cost a player their
        // first launch.** The PlayStation Portable's sysroot overlay reports
        // `ErrorKind::Uncategorized` for a missing file from both `read` and
        // `metadata` — so `Path::exists` is no help either — and a cartridge
        // copied to a memory stick with no save beside it yet refused to start
        // at all: *"could not read the existing save file"*, for a file that
        // was never there. Measured on the real target, `platform/psp/`.
        //
        // The directory listing is the one question that does work there, so
        // ask that. The safety property is untouched: a save whose **name is on
        // disk** and which still will not open is a hard error exactly as
        // before, and so is a directory we cannot even list.
        Err(_) if !listed_in_parent(path) => Ok(None),
        Err(_) => Err("could not read the existing save file"),
    }
}

/// Is `path`'s file name present in its directory?
///
/// Deliberately **fails towards "yes"**: an unlistable directory, or a path
/// with no parent, answers `true`, so [`read_file`]'s caller still refuses to
/// overwrite a save it could not read. Compared case-insensitively because the
/// filesystem this exists for is a FAT memory stick, and because a false "yes"
/// only ever means one more honest read attempt.
fn listed_in_parent(path: &path::Path) -> bool {
    let Some(name) = path.file_name() else {
        return true;
    };
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => path::Path::new("."),
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    entries
        .flatten()
        .any(|e| e.file_name().eq_ignore_ascii_case(name))
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

/// The standard MBC3 RTC footer: 10 × `u32` + `u64`, all little-endian.
/// This is what FlashGBX writes when it dumps a cartridge, and what VBA-M and
/// BGB read. See the module docs for the field layout.
pub const RTC_BYTES: usize = 48;

/// An older variant with a 32-bit timestamp. Read, never written.
pub const RTC_BYTES_32: usize = 44;

fn encode_rtc(state: &RtcState) -> Vec<u8> {
    let mut out = Vec::with_capacity(RTC_BYTES);
    // Each 8-bit register is stored widened to u32, upper three bytes zero.
    // The HALT (bit 6) and day-CARRY (bit 7) flags in DH must survive — other
    // emulators read them, even though some cartridge writers drop them.
    for v in state.regs.iter().chain(state.latched.iter()) {
        out.extend_from_slice(&(*v as u32).to_le_bytes());
    }
    out.extend_from_slice(&state.timestamp.to_le_bytes());
    out
}

fn decode_rtc(bytes: &[u8]) -> Option<RtcState> {
    if bytes.len() != RTC_BYTES && bytes.len() != RTC_BYTES_32 {
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
    let timestamp = if bytes.len() == RTC_BYTES {
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&bytes[40..48]);
        u64::from_le_bytes(ts)
    } else {
        let mut ts = [0u8; 4];
        ts.copy_from_slice(&bytes[40..44]);
        u32::from_le_bytes(ts) as u64
    };
    Some(RtcState {
        regs,
        latched,
        timestamp,
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
        let mut b = Battery::new(&rom, 0x2000, false);
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

        let loaded = Battery::new(&rom, 0x2000, false).load().unwrap();
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

        let mut b = Battery::new(&rom, 0x2000, false);
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
        let loaded = Battery::new(&rom, 0x2000, false).load().unwrap();
        assert_eq!(loaded.ram.len(), 0x2000);
        assert_eq!(&loaded.ram[..16], &[7u8; 16]);
        assert_eq!(loaded.ram[16], 0);

        fs::write(dir.join("game.sav"), vec![3u8; 0x4000]).unwrap();
        let loaded = Battery::new(&rom, 0x2000, false).load().unwrap();
        assert_eq!(loaded.ram, vec![3u8; 0x2000]);
    }

    #[test]
    fn a_clean_boot_never_creates_a_save_file() {
        let dir = tmpdir("noboot");
        let rom = dir.join("game.gb");
        let mut b = Battery::new(&rom, 0x2000, false);
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
        let mut b = Battery::new(&rom, 0x2000, false);
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
        let mut b = Battery::new(&rom, 0x2000, false);
        assert!(b.load().is_err());
        b.touch();
        b.flush(&vec![0u8; 0x2000], None);
        assert!(!dir.join("game.sav").exists());
        assert_eq!(fs::read(dir.join("game.gbsave")).unwrap(), vec![1u8; 4]);
    }

    /// A directory listing answers "is there a save?" on a platform whose
    /// `std` cannot classify a missing file — and still refuses when the file
    /// is really there.
    ///
    /// The PSP's sysroot overlay reports `Uncategorized` for a missing file, so
    /// `read_file`'s `NotFound` arm never fires there and a first launch used to
    /// die on a save that did not exist. This pins both directions of the
    /// fallback without needing that platform.
    #[test]
    fn a_missing_save_is_absent_and_a_present_one_is_not() {
        let dir = tmpdir("listed");
        assert!(!listed_in_parent(&dir.join("nothing.sav")));
        fs::write(dir.join("something.sav"), b"x").unwrap();
        assert!(listed_in_parent(&dir.join("something.sav")));
        // A FAT memory stick is case-insensitive, and the PSP's directory
        // listing does not preserve the case we asked with.
        assert!(listed_in_parent(&dir.join("SOMETHING.SAV")));
        // No parent, and an unlistable directory, both fail towards "present"
        // so the caller keeps refusing rather than overwriting.
        assert!(listed_in_parent(path::Path::new("")));
        assert!(listed_in_parent(&dir.join("no-such-dir").join("x.sav")));
    }

    /// The exact byte layout FlashGBX writes and VBA-M/BGB read. Pinned so a
    /// future refactor cannot quietly change what we hand to other tools.
    #[test]
    fn rtc_footer_matches_the_ecosystem_layout() {
        let bytes = encode_rtc(&RtcState {
            regs: [0x01, 0x02, 0x03, 0x04, 0xC1],
            latched: [0x11, 0x12, 0x13, 0x14, 0x81],
            timestamp: 0x0102_0304_0506_0708,
        });
        assert_eq!(bytes.len(), 48);
        // Five registers, each widened to u32 LE with the upper bytes zero.
        assert_eq!(
            &bytes[0x00..0x14],
            &[
                0x01, 0, 0, 0, 0x02, 0, 0, 0, 0x03, 0, 0, 0, 0x04, 0, 0, 0, 0xC1, 0, 0,
                0,
            ]
        );
        // Then the latched copy.
        assert_eq!(
            &bytes[0x14..0x28],
            &[
                0x11, 0, 0, 0, 0x12, 0, 0, 0, 0x13, 0, 0, 0, 0x14, 0, 0, 0, 0x81, 0, 0,
                0,
            ]
        );
        // Then a u64 LE timestamp.
        assert_eq!(&bytes[0x28..0x30], &[8, 7, 6, 5, 4, 3, 2, 1]);
        // DH bit 6 (HALT) and bit 7 (day CARRY) must survive the round trip —
        // some cartridge writers drop them, but emulators read them.
        let back = decode_rtc(&bytes).unwrap();
        assert_eq!(back.regs[4], 0xC1);
        assert_eq!(back.latched[4], 0x81);
    }

    /// An older 44-byte variant stores a 32-bit timestamp. Read, never written.
    #[test]
    fn reads_the_44_byte_rtc_variant() {
        let mut bytes = encode_rtc(&RtcState {
            regs: [1, 2, 3, 4, 5],
            latched: [1, 2, 3, 4, 5],
            timestamp: 0,
        });
        bytes.truncate(40);
        bytes.extend_from_slice(&7777u32.to_le_bytes());
        let s = decode_rtc(&bytes).unwrap();
        assert_eq!(s.timestamp, 7777);
        assert_eq!(s.regs, [1, 2, 3, 4, 5]);
    }

    /// A cartridge with no clock must get no footer: an unexpected 48 trailing
    /// bytes is exactly what made our old saves unreadable elsewhere.
    #[test]
    fn a_clockless_cart_writes_no_footer() {
        let dir = tmpdir("nofooter");
        let rom = dir.join("game.gb");
        let mut b = Battery::new(&rom, 0x2000, false);
        b.touch();
        b.flush(&vec![0x42u8; 0x2000], None);
        assert_eq!(fs::read(dir.join("game.sav")).unwrap().len(), 0x2000);
    }

    /// A clock cart appends the footer to the same file, and reloads from it.
    #[test]
    fn an_rtc_cart_round_trips_through_one_file() {
        let dir = tmpdir("footer");
        let rom = dir.join("game.gb");
        let state = RtcState {
            regs: [5, 4, 3, 2, 0x41],
            latched: [5, 4, 3, 2, 0x41],
            timestamp: 1_600_000_000,
        };
        let mut b = Battery::new(&rom, 0x2000, true);
        b.touch();
        b.flush(&vec![0x99u8; 0x2000], Some(state));

        let written = fs::read(dir.join("game.sav")).unwrap();
        assert_eq!(written.len(), 0x2000 + 48);
        assert!(
            !dir.join("game.rtc").exists(),
            "no sidecar: the footer is in the .sav"
        );

        let loaded = Battery::new(&rom, 0x2000, true).load().unwrap();
        assert_eq!(loaded.ram, vec![0x99u8; 0x2000]);
        assert_eq!(loaded.rtc, Some(state));
    }

    /// `.srm` is the other extension in common use; accept it on load.
    #[test]
    fn reads_an_srm_file() {
        let dir = tmpdir("srm");
        let rom = dir.join("game.gb");
        fs::write(dir.join("game.srm"), vec![0x33u8; 0x2000]).unwrap();
        let loaded = Battery::new(&rom, 0x2000, false).load().unwrap();
        assert_eq!(loaded.ram, vec![0x33u8; 0x2000]);
    }

    /// The save-destroying case this whole latch exists for: the machine has
    /// faulted, so whatever is in cartridge RAM now is a crashed CPU's
    /// scribbling, not a save. The good file already on disk must survive.
    #[test]
    fn a_faulted_machine_never_overwrites_a_good_save() {
        let dir = tmpdir("fault");
        let rom = dir.join("game.gb");
        let good = vec![0xA5u8; 0x2000];

        let mut b = Battery::new(&rom, 0x2000, false);
        b.touch();
        b.flush(&good, None);
        assert_eq!(fs::read(dir.join("game.sav")).unwrap(), good);

        b.poison("the CPU left the program");
        // Everything that can write: the per-frame autosave, an explicit flush,
        // and the last-chance flush in `Drop`.
        let garbage = vec![0x5Au8; 0x2000];
        b.touch();
        for _ in 0..(MAX_DIRTY_FRAMES + 1) {
            b.tick(&garbage, None);
        }
        b.flush(&garbage, None);
        drop(b);

        assert_eq!(
            fs::read(dir.join("game.sav")).unwrap(),
            good,
            "a crashed machine must not be able to reach the save file"
        );
    }

    /// The first reason is the diagnosis; later ones are its consequences.
    #[test]
    fn poisoning_keeps_the_first_reason() {
        let mut b = Battery::none();
        assert_eq!(b.poisoned_reason(), None);
        b.poison("undefined opcode");
        b.poison("and then everything else went wrong");
        assert_eq!(b.poisoned_reason(), Some("undefined opcode"));
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

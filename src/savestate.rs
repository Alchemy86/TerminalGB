//! Save states for the whole machine.
//!
//! The core serialises the *complete* emulator state to a flat `Vec<u8>` and
//! restores it from `&[u8]`. It makes no filesystem assumptions, so every
//! frontend decides where the bytes live: the terminal writes a file, the
//! browser uses local storage or a download, the debug surface keeps them in
//! memory. The format is little-endian throughout and works identically on
//! native and `wasm32`.
//!
//! ## Format: `GBSS` (native, versioned) — not BESS
//!
//! We ship our own container, tagged `GBSS` and carrying [`FORMAT_VERSION`].
//! It is deliberately *not* SameBoy's BESS. BESS is a great interchange format,
//! but adopting it here would be a large job for little near-term gain and it
//! does not model this particular core's internal counters (PPU `modeclock`,
//! the timer's split `internaldiv`/`internalcnt`, our HDMA bookkeeping, the APU
//! frame-sequencer phase). BESS's "best effort" contract means a foreign loader
//! reconstructs those from the register file, which is exactly the class of
//! subtle desync this task warns against. So: **native versioned format now,
//! BESS import/export proposed as a follow-up** (it is purely additive — a
//! `bess.rs` that maps our fields to/from BESS blocks, gated behind a feature).
//! See `docs/savestates.md` for the full verdict and the block layout.
//!
//! ## Safety rails
//! * A 4-byte magic and a `u16` version are checked first; a version mismatch
//!   fails loudly ([`StateError::UnsupportedVersion`]) rather than loading
//!   garbage.
//! * The ROM is **never** stored. We store the cartridge title and its global
//!   checksum and refuse to load a state against a different game
//!   ([`StateError::RomMismatch`]).
//! * Variable-length and platform-varying sections (APU, MBC RAM) are
//!   length-prefixed so a reader can skip what it cannot use — that is what lets
//!   a state saved on a machine that had a sound chip load into one that did not.
//!   (`wasm32` builds emulate the APU now too, and the format needed no change
//!   when that happened, which is the point.)

use std::fmt;

/// Container tag. Bumped only on an incompatible container change.
pub const MAGIC: [u8; 4] = *b"GBSS";

/// State format version. **Bump on any change to the serialised layout.** A
/// state written by a different version is rejected rather than mis-parsed.
/// v2: APU register shadow (`Sound::regs`, 22 bytes) + `dmg_mode` flag added
/// for hardware-accurate NRxx read-back (dmg_sound 01-registers).
/// v3: `WaveChannel::since_fetch` (DMG wave-RAM access window).
/// v4: the timer became the hardware's single 16-bit system counter — `div`
/// (u16) plus the TIMA-overflow reload latches — instead of two independent
/// divisors (Mooneye `acceptance/timer/*`).
/// v5: APU sample-accuracy state — the double-speed clock divider
/// (`Sound::double_speed`, `cpu_frac`, so CGB double-speed audio does not come
/// back an octave high), the square duty output latch, the 1 MHz grid phase and
/// the pending DIV-APU power-on skip (SameSuite `apu/*`).
/// v6: VRAM DMA — the four HDMA1-4 register bytes are gone (they were a shadow
/// of the address counters, which are the real registers and are what the
/// transfer advances), and the "move a block now" one-shot an HBlank transfer
/// armed during mode 0 sets is captured (SameSuite `dma/*`).
/// v7: APU corner cases — the sweep unit's pending-calculation timer (and
/// whether it was armed with a zero shift) and the volume envelope's
/// pending-step latch, all three of which can be live at a frame boundary
/// (SameSuite `channel_1_sweep`, `channel_N_nrx2_speed_change`); plus the PPU's
/// STAT interrupt line (`Gpu::stat_line`), the latched `LY==LYC` coincidence
/// flip-flop (`lyc_match`) and the "no OAM scan on the line the PPU restarts
/// on" latch (`skip_first_oam`) — all three survive a frame boundary too, so a
/// state that dropped them would resume with a stale STAT edge (Mooneye
/// `acceptance/ppu/stat_irq_blocking`, `stat_lyc_onoff`).
/// v8: `STOP` became the real instruction — `Cpu::speed_switch_halt` (the
/// ~0.93-frame HALT a CGB speed switch runs through) and `Gpu::stop_fill` (the
/// blanked panel of a stopped CPU), both of which can be live at a frame
/// boundary (`daid/stop_instr`, `daid/speed_switch_timing_*`).
/// v9: the MBC3 real-time clock became a counter chain driven by emulated time —
/// the block is now the five counters, the latch snapshot and the **sub-second
/// divider** (which is live machine state: drop it and every load moves the next
/// tick by up to a second) instead of `rtc_ram`/`rtc_ram_latch`/`rtc_zero`
/// (ax6 `rtc3test`).
/// v10: the link port's shift register — an internally clocked transfer is a
/// timed 8-shift-clock affair now, so the shifts still owed and the byte due to
/// land in `SB` both survive across a frame boundary (Mooneye
/// `acceptance/serial/boot_sclk_align`).
/// v11: `get_mbc` stopped forcing an MBC3 when no save path was given, so a
/// machine built with `Gameboy::new(rom, None)` — the FFI and wasm builds, and
/// any embedder — now writes the block of the mapper its cartridge header
/// actually asks for. Same cartridge, different MBC block: an older state from
/// those builds would be mis-parsed, so it is refused instead. States written
/// by the desktop and terminal frontends (which always pass a path) were
/// unaffected by that fix, but the container version is global.
/// v12: the PPU's pixel pipeline. A state taken part-way through mode 3 has to
/// resume the same scanline — mid-tile-fetch, with both FIFOs and the sprite
/// scan list intact — so the whole of it is now in the GPU block. Written
/// whichever renderer is running, so a state moves freely between the two.
/// v13: channel 1's sweep unit gained the two latches its delayed overflow
/// check actually compares — the addend latched when the check was armed, and
/// the post-trigger restart hold during which the shadow reads zero. Both can
/// be live at a frame boundary (SameSuite `channel_1_sweep_restart_2`).
/// v14: the Super Game Boy arrived — a length-prefixed, present-flagged SGB
/// block sits between the joypad and the PPU, carrying the ICD2's packet
/// receiver and the multiplayer joypad index ([`crate::sgb`]). It is skippable
/// like the APU block, so a state taken on an SGB still loads on a build that
/// is not one, but its position in the stream moved every later block.
/// v15: each square channel now carries how long ago its frequency timer last
/// reloaded, which decides whether an NRx3/NRx4 write still edits that reload
/// (SameSuite `channel_1_freq_change_timing-cgbDE`). It cannot be derived from
/// the countdown, and it is live at a frame boundary.
/// v16: the per-dot PPU's object fetch gained the two fields Pan Docs' OBJ
/// penalty algorithm needs — the stall dots still owed before the fetch's
/// remaining reads may run, and which background tile has already paid its
/// per-tile penalty on this line. Both are live mid-mode-3, which is exactly
/// where `testharness/savestate.rs` takes its snapshots.
/// v17: whether a user-supplied boot ROM is still mapped over the cartridge
/// ([`crate::boot`]). The image itself is never stored — it is the host's file,
/// like the cartridge — so a state taken during the boot animation refuses to
/// load into a machine that has none rather than resuming into the cartridge at
/// `PC=$0000`.
pub const FORMAT_VERSION: u16 = 19;

/// Everything that can go wrong loading a state. Loads fail loudly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The blob does not start with [`MAGIC`].
    BadMagic,
    /// The blob's version is not [`FORMAT_VERSION`].
    UnsupportedVersion(u16),
    /// The state belongs to a different cartridge.
    RomMismatch { expected: String, found: String },
    /// The blob ended before a field could be read.
    Truncated,
    /// A field held a value the current build cannot represent.
    BadField(&'static str),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::BadMagic => write!(f, "not a GBSS save state (bad magic)"),
            StateError::UnsupportedVersion(v) => write!(
                f,
                "unsupported save-state version {v} (this build writes v{FORMAT_VERSION})"
            ),
            StateError::RomMismatch { expected, found } => write!(
                f,
                "save state is for a different game (state: {found:?}, loaded ROM: {expected:?})"
            ),
            StateError::Truncated => write!(f, "save state is truncated"),
            StateError::BadField(name) => {
                write!(f, "save state has an invalid value for field '{name}'")
            }
        }
    }
}

impl std::error::Error for StateError {}

/// Append-only little-endian byte sink. No allocations beyond the backing Vec.
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    #[inline]
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    #[inline]
    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    #[inline]
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    #[inline]
    pub fn usize(&mut self, v: usize) {
        self.u32(v as u32);
    }
    /// Raw bytes, no length prefix (caller knows the length).
    #[inline]
    pub fn bytes(&mut self, s: &[u8]) {
        self.buf.extend_from_slice(s);
    }
    /// Length-prefixed byte block. A reader can skip it without understanding it.
    #[inline]
    pub fn blob(&mut self, s: &[u8]) {
        self.u32(s.len() as u32);
        self.bytes(s);
    }
    /// Length-prefixed string.
    #[inline]
    pub fn string(&mut self, s: &str) {
        self.blob(s.as_bytes());
    }
    /// `Option<u64>` as a presence flag + value.
    #[inline]
    pub fn opt_u64(&mut self, v: Option<u64>) {
        match v {
            Some(x) => {
                self.bool(true);
                self.u64(x);
            }
            None => self.bool(false),
        }
    }
}

/// Cursor over a byte slice. Every read is bounds-checked and returns
/// [`StateError::Truncated`] on underflow, so a malformed blob never panics.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        let end = self.pos.checked_add(n).ok_or(StateError::Truncated)?;
        let s = self.buf.get(self.pos..end).ok_or(StateError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    #[inline]
    pub fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }
    #[inline]
    pub fn bool(&mut self) -> Result<bool, StateError> {
        Ok(self.u8()? != 0)
    }
    #[inline]
    pub fn u16(&mut self) -> Result<u16, StateError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    #[inline]
    pub fn u32(&mut self) -> Result<u32, StateError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    #[inline]
    pub fn u64(&mut self) -> Result<u64, StateError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    #[inline]
    pub fn i32(&mut self) -> Result<i32, StateError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    #[inline]
    pub fn usize(&mut self) -> Result<usize, StateError> {
        Ok(self.u32()? as usize)
    }
    /// Read `dst.len()` bytes into `dst`.
    #[inline]
    pub fn fill(&mut self, dst: &mut [u8]) -> Result<(), StateError> {
        let n = dst.len();
        dst.copy_from_slice(self.take(n)?);
        Ok(())
    }
    /// Read a length-prefixed byte block (as written by [`Writer::blob`]).
    #[inline]
    pub fn blob(&mut self) -> Result<Vec<u8>, StateError> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    #[inline]
    pub fn string(&mut self) -> Result<String, StateError> {
        let b = self.blob()?;
        String::from_utf8(b).map_err(|_| StateError::BadField("utf8 string"))
    }
    #[inline]
    pub fn opt_u64(&mut self) -> Result<Option<u64>, StateError> {
        if self.bool()? {
            Ok(Some(self.u64()?))
        } else {
            Ok(None)
        }
    }
    /// Consume and verify the 4-byte magic.
    pub fn expect_magic(&mut self) -> Result<(), StateError> {
        if self.take(4)? == MAGIC {
            Ok(())
        } else {
            Err(StateError::BadMagic)
        }
    }
}

/// FNV-1a 64-bit hash — used by the round-trip test to compare full-machine
/// snapshots and framebuffers cheaply. Kept here so tests and any debug tooling
/// share one implementation. (The test harness hand-rolls the same primitive;
/// this is the core-side copy so no dependency is added.)
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

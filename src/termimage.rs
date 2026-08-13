//! Terminal image transmission at a **capped** resolution.
//!
//! # Why this exists
//!
//! The terminal frontend used to hand `ratatui-image` a `Resize::Scale`
//! request, which resizes the source image to *the pixel size of the display
//! rectangle* and then encodes and transmits that. The cost of every displayed
//! frame therefore scaled with the window: on a large window the emulator was
//! PNG-encoding (iTerm2) or base64-ing (Kitty) two million pixels sixty times a
//! second. Measured (see `examples/render_bench.rs` and
//! [`docs/terminal-render.md`](../docs/terminal-render.md)), a 1568×1408
//! display rectangle costs **39.9 ms and 11.6 MB per frame** on Kitty and
//! **84.8 ms** on iTerm2 — 25 fps and 12 fps respectively, before anything else
//! happens. The same picture transmitted at 160×144 costs **0.10 ms and 92 KB**,
//! and does not change when the window grows. That is the whole bug.
//!
//! Both protocols implemented here can be told to **place an image over a
//! rectangle measured in character cells**, letting the terminal do the final
//! stretch:
//!
//! * **Kitty graphics protocol** — a virtual placement (`U=1`) carrying `c=`
//!   (columns) and `r=` (rows) is scaled by the terminal to exactly that cell
//!   rectangle, and is positioned with Unicode placeholder cells.
//!   <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>
//! * **iTerm2 inline images** — `width=`/`height=` default to *character cells*
//!   (`Npx` would mean pixels), so the terminal scales the image to the cell
//!   rectangle. <https://iterm2.com/documentation-images.html>
//!
//! So the transmitted image is sized by *what the emulator computed* (the
//! pixel-art upscaler's own output), never by how big the window is. Cost
//! becomes constant with window size, which is the point.
//!
//! # No dependencies
//!
//! Same ethos as `savestate.rs` and the test harness: base64 and the PNG
//! container are hand-rolled here (the PNG uses zlib *stored* blocks — no
//! compression, hence no compression cost, which is what we want at 60 Hz), so
//! this module is pure `std`, builds on `wasm32`, and adds nothing to
//! `Cargo.toml`.
//!
//! # Why the sequence is emitted outside `ratatui`'s cell buffer
//!
//! [`TermImage::place`] returns one **absolutely positioned** escape sequence
//! for the caller to write to the terminal directly, rather than stuffing it
//! into a cell's symbol the way `ratatui-image` does. That is not a stylistic
//! choice — putting it in a cell is actively broken at this size:
//!
//! * `Buffer::diff` computes `invalidated = max(symbol.width(), invalidated)`
//!   and re-emits every following cell while that counter is non-zero. A 92 KB
//!   base64 payload has a display width of ~92 000, so **the entire rest of the
//!   screen is re-emitted every single frame** — measured at 84 KB per frame for
//!   a picture that had not changed at all.
//! * Anything drawn over those cells (a stats overlay in the corner, say)
//!   replaces the symbol that carries the image, so the picture silently stops
//!   updating until the overlay is dismissed.
//!
//! Emitting it ourselves, after the frame is drawn, fixes both: the cells the
//! image covers are marked `skip` so `ratatui` never touches them, and the
//! image is transmitted only when it actually changed.
//!
//! # Unchanged-frame skipping
//!
//! [`TermImage::place`] hashes the pixels it is given and reports
//! `changed: false` when neither the frame nor the placement moved — the caller
//! then writes nothing at all. A still frame (menu, dialogue box, pause) costs
//! zero encode and zero bytes on the wire.
//!
//! The Kitty placeholder layout follows `ratatui-image` (MIT, Benjamin Große),
//! which is the arrangement already proven to work in this frontend; the
//! transmission (RGB rather than RGBA, explicit `c`/`r` scaling, absolute
//! positioning, buffer reuse) is ours.

use std::fmt::Write as _;

/// Which terminal graphics protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Kitty graphics protocol with Unicode placeholders.
    Kitty,
    /// iTerm2 inline images (also implemented by WezTerm).
    Iterm2,
}

/// The largest placement Kitty's row/column diacritics can address.
pub const MAX_CELLS: u16 = 297;

/// Where the picture goes, in terminal cells: the top-left cell (zero-based)
/// and the size of the rectangle the terminal will scale the image to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

impl CellRect {
    pub fn new(x: u16, y: u16, cols: u16, rows: u16) -> CellRect {
        CellRect { x, y, cols, rows }
    }

    /// Clamp to what Kitty's row/column diacritics can address.
    fn clamped(self) -> CellRect {
        CellRect {
            cols: self.cols.clamp(1, MAX_CELLS),
            rows: self.rows.clamp(1, MAX_CELLS),
            ..self
        }
    }
}

/// A prepared placement: one escape sequence, ready to write to the terminal.
///
/// The sequence positions itself (it saves the cursor, moves to the placement's
/// absolute cell, draws, and restores), so the caller writes it verbatim after
/// the rest of the frame has been flushed. When `changed` is false the sequence
/// is empty and **nothing should be written**.
pub struct Placement<'a> {
    /// The bytes to send, or empty when the picture is unchanged.
    pub seq: &'a str,
    /// `true` if the image payload was re-encoded and must be retransmitted.
    pub changed: bool,
    /// Length of `seq`.
    pub payload_bytes: usize,
}

/// Encoder + cache for one terminal image slot.
pub struct TermImage {
    wire: Wire,
    /// Kitty image id. Kept below 2^24 so the "most significant byte" diacritic
    /// is never needed.
    id: u32,
    // Reusable buffers — nothing here allocates per frame in the steady state.
    rgb: Vec<u8>,
    png: Vec<u8>,
    b64: String,
    /// The full sequence for the current frame.
    seq: String,
    /// Kitty: the placeholder grid, which depends only on the placement and so
    /// is rebuilt on a resize rather than every frame.
    placeholders: String,
    /// What the cached sequence was built from.
    key: Option<Key>,
    /// Set when the caller wants the next `place` to rebuild regardless.
    force: bool,
}

#[derive(PartialEq, Eq, Clone, Copy)]
struct Key {
    hash: u64,
    w: u32,
    h: u32,
    at: CellRect,
}

impl TermImage {
    pub fn new(wire: Wire, id: u32) -> TermImage {
        TermImage {
            wire,
            id: id & 0x00ff_ffff,
            rgb: Vec::new(),
            png: Vec::new(),
            b64: String::new(),
            seq: String::new(),
            placeholders: String::new(),
            key: None,
            force: false,
        }
    }

    pub fn wire(&self) -> Wire {
        self.wire
    }

    /// Drop the cache so the next [`TermImage::place`] re-encodes and
    /// retransmits even for an identical frame. Needed after anything that can
    /// wipe the terminal's copy of the image (a full redraw, leaving an
    /// overlay, a protocol switch).
    pub fn invalidate(&mut self) {
        self.force = true;
    }

    /// Encode `rgba` (`w`×`h`, 4 bytes per pixel) for display over the
    /// `cols`×`rows` cell rectangle whose top-left cell is `(x, y)`
    /// (zero-based), reusing the previous encoding when nothing changed.
    ///
    /// The transmitted image is `w`×`h` — **the display rectangle only ever
    /// affects the placement, never the payload size.**
    pub fn place(&mut self, rgba: &[u8], w: u32, h: u32, at: CellRect) -> Placement<'_> {
        debug_assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);
        let at = at.clamped();
        let key = Key {
            hash: fnv1a(rgba),
            w,
            h,
            at,
        };
        if !self.force && self.key == Some(key) && !self.seq.is_empty() {
            return Placement {
                seq: "",
                changed: false,
                payload_bytes: 0,
            };
        }
        // A move or resize invalidates the placeholder grid too; a plain new
        // frame reuses it.
        let geometry_changed = self.force
            || self.placeholders.is_empty()
            || !matches!(self.key, Some(k) if k.at == at);
        self.force = false;
        self.key = Some(key);

        match self.wire {
            Wire::Kitty => self.build_kitty(rgba, w, h, at, geometry_changed),
            Wire::Iterm2 => self.build_iterm2(rgba, w, h, at),
        }
        Placement {
            seq: &self.seq,
            changed: true,
            payload_bytes: self.seq.len(),
        }
    }

    // -- Kitty ------------------------------------------------------------

    /// Transmit the image, then draw the Unicode-placeholder grid that places
    /// it. `c=`/`r=` make the terminal scale the image to the cell rectangle,
    /// which is what decouples payload size from window size.
    fn build_kitty(
        &mut self,
        rgba: &[u8],
        w: u32,
        h: u32,
        at: CellRect,
        geometry_changed: bool,
    ) {
        let CellRect { x, y, cols, rows } = at;
        rgba_to_rgb(rgba, &mut self.rgb);
        base64_into(&self.rgb, &mut self.b64);

        if geometry_changed {
            // Foreground colour carries the low three bytes of the image id;
            // each row is positioned absolutely, and the first cell of a row
            // carries its row/column diacritics (the rest inherit).
            let [_, r, g, b] = self.id.to_be_bytes();
            self.placeholders.clear();
            for row in 0..rows {
                let _ = write!(
                    self.placeholders,
                    "\x1b[{};{}H\x1b[38;2;{r};{g};{b}m",
                    y + row + 1,
                    x + 1
                );
                self.placeholders.push('\u{10EEEE}');
                self.placeholders.push(diacritic(row));
                self.placeholders.push(diacritic(0));
                for _ in 1..cols {
                    self.placeholders.push('\u{10EEEE}');
                }
            }
        }

        self.seq.clear();
        // DECSC/DECRC: save and restore the cursor *and* its attributes, so the
        // colour used to carry the image id never leaks into the rest of the UI.
        self.seq.push_str("\x1b[s");
        // Transmit + virtual-place, chunked at 4096 base64 bytes as the
        // protocol requires.
        let id = self.id;
        let bytes = self.b64.as_bytes();
        let n = bytes.chunks(4096).len().max(1);
        if bytes.is_empty() {
            let _ = write!(
                self.seq,
                "\x1b_Gq=2,i={id},a=T,U=1,f=24,t=d,s={w},v={h},c={cols},r={rows},m=0;\x1b\\"
            );
        }
        for (i, chunk) in bytes.chunks(4096).enumerate() {
            // base64 output is ASCII, so chunking bytes never splits a char.
            let payload = std::str::from_utf8(chunk).unwrap_or("");
            let more = u8::from(i + 1 < n);
            if i == 0 {
                let _ = write!(
                    self.seq,
                    "\x1b_Gq=2,i={id},a=T,U=1,f=24,t=d,s={w},v={h},c={cols},r={rows},m={more};{payload}\x1b\\"
                );
            } else {
                let _ = write!(self.seq, "\x1b_Gq=2,m={more};{payload}\x1b\\");
            }
        }
        self.seq.push_str(&self.placeholders);
        self.seq.push_str("\x1b[u");
    }

    // -- iTerm2 -----------------------------------------------------------

    fn build_iterm2(&mut self, rgba: &[u8], w: u32, h: u32, at: CellRect) {
        let CellRect { x, y, cols, rows } = at;
        rgba_to_rgb(rgba, &mut self.rgb);
        png_rgb_into(&self.rgb, w, h, &mut self.png);
        base64_into(&self.png, &mut self.b64);

        self.seq.clear();
        self.seq.push_str("\x1b[s");
        let _ = write!(self.seq, "\x1b[{};{}H", y + 1, x + 1);
        // Erase the cells the image will cover first: with transparency the
        // terminal otherwise leaves stale glyphs behind the picture.
        for _ in 0..rows {
            let _ = write!(self.seq, "\x1b[{cols}X\x1b[1B");
        }
        let _ = write!(self.seq, "\x1b[{};{}H", y + 1, x + 1);
        // `width`/`height` without a unit are **character cells** — the terminal
        // scales the image for us, so the payload stays small.
        let _ = write!(
            self.seq,
            "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=0;doNotMoveCursor=1:{}\x07",
            self.png.len(),
            self.b64,
        );
        self.seq.push_str("\x1b[u");
    }
}

/// Drop the alpha channel in place into `out` (both protocols here are fed
/// opaque frames; RGB is 25 % less to base64 than RGBA).
fn rgba_to_rgb(rgba: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(rgba.len() / 4 * 3);
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&px[..3]);
    }
}

// ---------------------------------------------------------------------------
// base64
// ---------------------------------------------------------------------------

const B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, written into a reused `String`.
pub fn base64_into(input: &[u8], out: &mut String) {
    out.clear();
    out.reserve(input.len().div_ceil(3) * 4);
    // Safe: we only push ASCII bytes from the base64 alphabet.
    let buf = unsafe { out.as_mut_vec() };
    let mut chunks = input.chunks_exact(3);
    for c in &mut chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
        buf.push(B64[(n >> 18) as usize & 63]);
        buf.push(B64[(n >> 12) as usize & 63]);
        buf.push(B64[(n >> 6) as usize & 63]);
        buf.push(B64[n as usize & 63]);
    }
    match chunks.remainder() {
        [a] => {
            let n = (*a as u32) << 16;
            buf.push(B64[(n >> 18) as usize & 63]);
            buf.push(B64[(n >> 12) as usize & 63]);
            buf.push(b'=');
            buf.push(b'=');
        }
        [a, b] => {
            let n = ((*a as u32) << 16) | ((*b as u32) << 8);
            buf.push(B64[(n >> 18) as usize & 63]);
            buf.push(B64[(n >> 12) as usize & 63]);
            buf.push(B64[(n >> 6) as usize & 63]);
            buf.push(b'=');
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Minimal PNG (RGB, zlib "stored" blocks — no compression, no compression cost)
// ---------------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in bytes {
        crc = (crc >> 8) ^ CRC_TABLE[((crc ^ b as u32) & 0xff) as usize];
    }
    !crc
}

static CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn adler32(bytes: &[u8]) -> u32 {
    // Deferred modulo: 5552 bytes is the most that cannot overflow u32.
    let (mut a, mut b): (u32, u32) = (1, 0);
    for block in bytes.chunks(5552) {
        for &byte in block {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode packed RGB bytes as an 8-bit RGB PNG into `out` (reused buffer).
///
/// Uses zlib *stored* (uncompressed) deflate blocks: ~0.03 % larger than the
/// raw bytes but with **no deflate cost**, which is the right trade at 60 Hz —
/// compressing a frame costs far more time than the extra bytes cost the pipe.
pub fn png_rgb_into(rgb: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    // Filter byte (0 = none) per scanline.
    let mut raw = Vec::with_capacity(h * (1 + w * 3));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * w * 3..(y + 1) * w * 3]);
    }

    let mut zlib = Vec::with_capacity(raw.len() + raw.len() / 0xffff * 5 + 16);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut pos = 0;
    while pos < raw.len() {
        let block = (raw.len() - pos).min(0xffff);
        let last = u8::from(pos + block >= raw.len());
        zlib.push(last);
        zlib.extend_from_slice(&(block as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + block]);
        pos += block;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    out.clear();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    let mut ihdr = [0u8; 13];
    ihdr[..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8..].copy_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, colour type 2 (RGB)
    png_chunk(out, b"IHDR", &ihdr);
    png_chunk(out, b"IDAT", &zlib);
    png_chunk(out, b"IEND", &[]);
}

// ---------------------------------------------------------------------------
// Hashing (frame-change detection)
// ---------------------------------------------------------------------------

/// FNV-1a over 8-byte words then the tail. Not cryptographic — it only has to
/// tell "this frame differs from the last one", and a 64-bit hash over a
/// 92 KiB frame collides far too rarely to matter visually.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let word = u64::from_ne_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        hash ^= word;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    for &b in chunks.remainder() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// Kitty row/column diacritics
// ---------------------------------------------------------------------------

/// Row/column encoding diacritics from the Kitty graphics protocol's
/// `rowcolumn-diacritics.txt` (the protocol's own data table). Index `n`
/// selects the combining mark that means "row/column `n`".
static DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A,
    0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365,
    0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F,
    0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
    0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 0x05A8, 0x05A9,
    0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6,
    0x06D7, 0x06D8, 0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
    0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736,
    0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 0x0745, 0x0747, 0x0749, 0x074A,
    0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817,
    0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951,
    0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD,
    0x193A, 0x1A17, 0x1A75, 0x1A76, 0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C,
    0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
    0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6,
    0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF,
    0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
    0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0,
    0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6,
    0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0,
    0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1,
    0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9,
    0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2,
    0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
    0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187, 0x1D188,
    0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// The combining mark encoding row/column `n` (clamped to the table).
fn diacritic(n: u16) -> char {
    let idx = (n as usize).min(DIACRITICS.len() - 1);
    char::from_u32(DIACRITICS[idx]).unwrap_or('\u{0305}')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, seed: u8) -> Vec<u8> {
        (0..w * h * 4)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    #[test]
    fn base64_matches_known_vectors() {
        let mut out = String::new();
        for (input, want) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            base64_into(input, &mut out);
            assert_eq!(out, want, "base64({input:?})");
        }
    }

    #[test]
    fn png_has_valid_container_and_size() {
        let rgb = vec![0x7fu8; 4 * 3 * 3];
        let mut png = Vec::new();
        png_rgb_into(&rgb, 4, 3, &mut png);
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
        );
        // IHDR length + tag + 13 bytes of data.
        assert_eq!(&png[8..12], &13u32.to_be_bytes());
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &4u32.to_be_bytes());
        assert_eq!(&png[20..24], &3u32.to_be_bytes());
        assert!(png.ends_with(b"\xaeB`\x82"), "IEND crc");
    }

    #[test]
    #[cfg(feature = "image")]
    fn png_decodes_back_to_the_same_pixels() {
        // Round-trip through the `image` crate's decoder to prove the stored
        // deflate stream is a real PNG, not just a plausible one.
        let rgb: Vec<u8> = (0..7 * 5 * 3).map(|i| (i * 7) as u8).collect();
        let mut png = Vec::new();
        png_rgb_into(&rgb, 7, 5, &mut png);
        let decoded = image::load_from_memory(&png).expect("valid PNG");
        assert_eq!((decoded.width(), decoded.height()), (7, 5));
        assert_eq!(decoded.to_rgb8().into_raw(), rgb);
    }

    /// Steady-state bytes for one *new* frame in a given rectangle: place once
    /// to settle the geometry, then place a different frame.
    fn steady_state_bytes(wire: Wire, cols: u16, rows: u16) -> usize {
        let mut t = TermImage::new(wire, 1);
        t.place(
            &frame(160, 144, 1),
            160,
            144,
            CellRect::new(0, 0, cols, rows),
        );
        t.place(
            &frame(160, 144, 2),
            160,
            144,
            CellRect::new(0, 0, cols, rows),
        )
        .payload_bytes
    }

    #[test]
    fn image_payload_is_independent_of_the_display_rectangle() {
        // The whole point: growing the window must not grow the *image*.
        // A 20×9 rectangle is a small window; 280×126 is a 4K terminal.
        //
        // Kitty additionally needs one placeholder character per covered cell
        // (4 UTF-8 bytes each), which is inherent to the protocol and linear in
        // cells; iTerm2 needs a short erase preamble. Everything beyond that
        // must be flat — against the old path, which grew with the *pixel* area
        // and reached 11.6 MB per frame at this size.
        for wire in [Wire::Kitty, Wire::Iterm2] {
            let small = steady_state_bytes(wire, 20, 9);
            let big = steady_state_bytes(wire, 280, 126);
            let inherent = match wire {
                Wire::Kitty => 280 * 126 * 4 + 280 * 16,
                Wire::Iterm2 => 4096,
            };
            assert!(
                big <= small + inherent,
                "{wire:?}: {small} -> {big} bytes/frame exceeds the protocol's own \
                 per-cell cost ({inherent}) — payload is following the window"
            );
            // And a 4K window still costs a fraction of one legacy frame.
            assert!(big < 400_000, "{wire:?}: {big} bytes/frame is too much");
        }
    }

    #[test]
    fn unchanged_frame_re_encodes_nothing() {
        for wire in [Wire::Kitty, Wire::Iterm2] {
            let img = frame(160, 144, 3);
            let mut t = TermImage::new(wire, 7);
            assert!(t.place(&img, 160, 144, CellRect::new(0, 0, 40, 18)).changed);
            let second = t.place(&img, 160, 144, CellRect::new(0, 0, 40, 18));
            assert!(!second.changed, "{wire:?} re-encoded an identical frame");
            assert_eq!(second.payload_bytes, 0);
            assert!(second.seq.is_empty(), "{wire:?} would still write bytes");
            // A different frame must be re-encoded.
            let img2 = frame(160, 144, 4);
            assert!(
                t.place(&img2, 160, 144, CellRect::new(0, 0, 40, 18))
                    .changed
            );
            // So must the same frame in a resized or moved rectangle.
            assert!(
                t.place(&img2, 160, 144, CellRect::new(0, 0, 41, 18))
                    .changed
            );
            assert!(
                t.place(&img2, 160, 144, CellRect::new(3, 1, 41, 18))
                    .changed
            );
            // And after an explicit invalidate — what the frontend calls when
            // something may have painted over the picture.
            t.invalidate();
            assert!(
                t.place(&img2, 160, 144, CellRect::new(3, 1, 41, 18))
                    .changed
            );
        }
    }

    #[test]
    fn kitty_transmits_cell_scaling_and_positions_every_row() {
        let img = frame(160, 144, 9);
        let mut t = TermImage::new(Wire::Kitty, 0x42);
        let p = t.place(&img, 160, 144, CellRect::new(4, 2, 30, 14));
        let seq = p.seq;
        assert!(seq.contains("a=T,U=1"), "virtual placement");
        assert!(seq.contains("s=160,v=144"), "source pixel size");
        assert!(seq.contains("c=30,r=14"), "terminal-side scaling to cells");
        assert!(seq.contains("f=24"), "RGB payload");
        // Positioned absolutely, one cursor move per row (1-based), and the
        // cursor is saved and restored around the whole thing.
        assert!(seq.starts_with("\x1b[s"), "cursor not saved");
        assert!(seq.ends_with("\x1b[u"), "cursor not restored");
        assert!(seq.contains("\x1b[3;5H"), "first row not placed at (4,2)");
        assert!(seq.contains("\x1b[16;5H"), "last row not placed");
        assert_eq!(
            seq.matches('\u{10EEEE}').count(),
            30 * 14,
            "placeholder grid does not cover the rectangle"
        );
    }

    #[test]
    fn iterm2_sizes_in_cells_not_pixels() {
        let img = frame(160, 144, 5);
        let mut t = TermImage::new(Wire::Iterm2, 1);
        let p = t.place(&img, 160, 144, CellRect::new(10, 3, 55, 25));
        let seq = p.seq;
        assert!(seq.contains("width=55;height=25"), "cell units, not px");
        assert!(!seq.contains("width=160px"));
        assert!(seq.contains("inline=1"));
        assert!(seq.contains("\x1b[4;11H"), "not positioned at (10,3)");
        assert!(seq.starts_with("\x1b[s") && seq.ends_with("\x1b[u"));
    }

    #[test]
    fn placement_is_clamped_to_addressable_cells() {
        let img = frame(160, 144, 2);
        let mut t = TermImage::new(Wire::Kitty, 1);
        let p = t.place(&img, 160, 144, CellRect::new(0, 0, 5000, 5000));
        assert!(p.seq.contains(&format!("c={MAX_CELLS},r={MAX_CELLS}")));
    }

    #[test]
    fn hash_distinguishes_frames() {
        assert_ne!(fnv1a(&frame(160, 144, 1)), fnv1a(&frame(160, 144, 2)));
        assert_eq!(fnv1a(&frame(160, 144, 1)), fnv1a(&frame(160, 144, 1)));
    }
}

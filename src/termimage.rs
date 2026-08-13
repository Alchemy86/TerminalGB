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
//! # …and compressed, because four colours are four colours
//!
//! Capping the transmitted *resolution* was only half the problem. The payload
//! was still raw RGB — 92,160 bytes per frame, ~123 KB after base64 — for a
//! picture with **four colours** in it. At 60 Hz that is 5.5 MB/s of escape
//! sequence for the terminal to parse and upload, and WezTerm managed about
//! **6 fps** on it while the emulator itself held a comfortable 59.7. Our own
//! encode was never the bottleneck; the wire was.
//!
//! Both protocols here accept PNG (Kitty `f=100`, iTerm2 natively), so the frame
//! is encoded as a **palette-indexed PNG** ([`PngEncoder`]) — two bits per pixel
//! for a four-colour frame — and deflated with [`crate::deflate`]. Measured on
//! real frames that is ~123 KB → ~1.5 KB on the wire for ~0.35 ms of encode
//! against a 16.7 ms frame. `GB_TERM_IMAGE=rgb` restores the raw Kitty payload
//! if some terminal's PNG decoder ever objects.
//!
//! # No dependencies
//!
//! Same ethos as `savestate.rs` and the test harness: base64, the PNG container
//! and the deflate compressor are all hand-rolled here, so this module is pure
//! `std`, builds on `wasm32`, and adds nothing to `Cargo.toml`.
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
    /// Escape hatch: send Kitty raw uncompressed RGB (`f=24`) instead of PNG.
    /// Only for a terminal whose PNG decoder turns out to reject palette
    /// images; costs ~60× the bytes. Set `GB_TERM_IMAGE=rgb`.
    raw_rgb: bool,
    /// Kitty only: place the image directly at the cursor instead of through a
    /// Unicode-placeholder grid. See [`kitty_direct_requested`].
    kitty_direct: bool,
    /// Kitty only: hand the frame over in shared memory (`t=s`) instead of
    /// base64 on the wire. See [`shm`].
    kitty_shm: bool,
    /// The shared-memory object the current sequence refers to. Held until the
    /// *next* frame replaces it: the terminal reads it asynchronously, after we
    /// return, so dropping it here would unlink it out from under the terminal.
    shm_live: Option<crate::shmframe::ShmFrame>,
    // Reusable buffers — nothing here allocates per frame in the steady state.
    rgb: Vec<u8>,
    png: Vec<u8>,
    enc: PngEncoder,
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
        let raw_rgb = raw_rgb_requested();
        let mut enc = PngEncoder::new();
        enc.set_uncompressed(raw_rgb);
        TermImage {
            wire,
            id: id & 0x00ff_ffff,
            raw_rgb,
            kitty_direct: kitty_direct_requested(),
            // `GB_TERM_KITTY=shm` forces shared memory without a handshake —
            // for measurement, and for someone who knows their terminal. The
            // frontend's normal route is to ask (see [`shm_query`]).
            kitty_shm: kitty_env_is("shm") && crate::shmframe::AVAILABLE,
            shm_live: None,
            rgb: Vec::new(),
            png: Vec::new(),
            enc,
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

    /// Use shared memory (`t=s`) for Kitty frames. Only ever turn this on after
    /// [`shm_query`] has come back `OK` from the terminal in front of you —
    /// `t=s` is optional in the protocol and a terminal that does not implement
    /// it shows nothing at all.
    pub fn set_kitty_shm(&mut self, on: bool) {
        self.kitty_shm = on && crate::shmframe::AVAILABLE;
        self.force = true;
    }

    pub fn kitty_shm(&self) -> bool {
        self.kitty_shm
    }

    /// Choose Kitty's placement mode explicitly, overriding the environment
    /// default (see [`kitty_direct_requested`]).
    pub fn set_kitty_placeholders(&mut self, on: bool) {
        self.kitty_direct = !on;
        self.force = true;
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

        // Best case: hand the pixels over in shared memory and send only the
        // object's name — about sixty bytes, whatever the picture is.
        if self.kitty_shm && self.build_kitty_shm(rgba, w, h, at) {
            return;
        }

        // `f=100` is PNG, which for a four-colour frame is ~60× less to send
        // than `f=24` raw RGB — and the terminal has to parse every byte of it.
        // <https://sw.kovidgoyal.net/kitty/graphics-protocol/#transferring-pixel-data>
        let fmt = if self.raw_rgb { 24 } else { 100 };
        if self.raw_rgb {
            rgba_to_rgb(rgba, &mut self.rgb);
            base64_into(&self.rgb, &mut self.b64);
        } else {
            self.enc.encode(rgba, w, h, &mut self.png);
            base64_into(&self.png, &mut self.b64);
        }

        if self.kitty_direct {
            // Direct placement: put the cursor where the image goes and let
            // `a=T` place it there, scaled to `c`×`r` cells. No placeholder
            // grid at all — which on a large window is 4 UTF-8 bytes per covered
            // cell (25 KB at 118×53), and by now that dwarfs the compressed
            // image itself. Re-transmitting under the same id deletes the old
            // placement, so nothing accumulates.
            self.seq.clear();
            self.seq.push_str("\x1b[s");
            let _ = write!(self.seq, "\x1b[{};{}H", y + 1, x + 1);
            self.write_kitty_payload(w, h, cols, rows, fmt, true);
            self.seq.push_str("\x1b[u");
            return;
        }

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
        self.write_kitty_payload(w, h, cols, rows, fmt, false);
        // The placeholder grid must be re-emitted every frame, not only when it
        // moves: "when re-transmitting image data for a specific id, the
        // existing image and all its placements must be deleted"
        // (kitty graphics protocol). Caching it in `placeholders` keeps the
        // *building* off the frame path; the bytes are unavoidable.
        self.seq.push_str(&self.placeholders);
        self.seq.push_str("\x1b[u");
    }

    /// Hand the frame over in shared memory and send only the object's name.
    ///
    /// The pixels are copied straight from the RGBA framebuffer with `f=32`, so
    /// there is no colour conversion, no PNG and no base64 — just a memcpy into
    /// a mapping the terminal will read. Returns `false` if shared memory is
    /// unavailable, in which case the caller falls back to a wire transport.
    ///
    /// Placement is always **direct** here: the point of `t=s` is that the
    /// sequence is tiny, and a Unicode-placeholder grid would put 4 bytes per
    /// covered cell back on the wire — 25 KB at a large window, dwarfing
    /// everything this saves.
    fn build_kitty_shm(&mut self, rgba: &[u8], w: u32, h: u32, at: CellRect) -> bool {
        let CellRect { x, y, cols, rows } = at;
        let Some(frame) =
            crate::shmframe::ShmFrame::new(rgba.len(), |m| m.copy_from_slice(rgba))
        else {
            return false;
        };
        base64_into(frame.name().as_bytes(), &mut self.b64);
        let id = self.id;
        self.seq.clear();
        self.seq.push_str("\x1b[s");
        let _ = write!(self.seq, "\x1b[{};{}H", y + 1, x + 1);
        // f=32 is RGBA, which is exactly our framebuffer's layout.
        let _ = write!(
            self.seq,
            "\x1b_Gq=2,i={id},a=T,f=32,t=s,s={w},v={h},c={cols},r={rows};{}\x1b\\",
            self.b64
        );
        self.seq.push_str("\x1b[u");
        // Replace the previous frame's object only now: the terminal reads
        // asynchronously, so the old mapping must outlive the write of the
        // sequence that referenced it.
        self.shm_live = Some(frame);
        true
    }

    /// Transmit the payload, chunked at 4096 base64 bytes as the protocol
    /// requires, carrying the placement keys on the first chunk. `direct`
    /// selects placement at the cursor over a virtual (`U=1`) placement.
    fn write_kitty_payload(
        &mut self,
        w: u32,
        h: u32,
        cols: u16,
        rows: u16,
        fmt: u8,
        direct: bool,
    ) {
        let id = self.id;
        let virt = if direct { "" } else { ",U=1" };
        let bytes = self.b64.as_bytes();
        let n = bytes.chunks(4096).len().max(1);
        if bytes.is_empty() {
            let _ = write!(
                self.seq,
                "\x1b_Gq=2,i={id},a=T{virt},f={fmt},t=d,s={w},v={h},c={cols},r={rows},m=0;\x1b\\"
            );
            return;
        }
        for (i, chunk) in bytes.chunks(4096).enumerate() {
            // base64 output is ASCII, so chunking bytes never splits a char.
            let payload = std::str::from_utf8(chunk).unwrap_or("");
            let more = u8::from(i + 1 < n);
            if i == 0 {
                let _ = write!(
                    self.seq,
                    "\x1b_Gq=2,i={id},a=T{virt},f={fmt},t=d,s={w},v={h},c={cols},r={rows},m={more};{payload}\x1b\\"
                );
            } else {
                let _ = write!(self.seq, "\x1b_Gq=2,m={more};{payload}\x1b\\");
            }
        }
    }

    // -- iTerm2 -----------------------------------------------------------

    fn build_iterm2(&mut self, rgba: &[u8], w: u32, h: u32, at: CellRect) {
        let CellRect { x, y, cols, rows } = at;
        self.enc.encode(rgba, w, h, &mut self.png);
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

/// `GB_TERM_IMAGE=rgb` restores the **pre-compression** payload — raw `f=24`
/// RGB on Kitty, uncompressed truecolour PNG on iTerm2. Two purposes: an escape
/// hatch if some terminal's PNG decoder ever rejects a palette image, and a
/// like-for-like "before" run for `render_bench --tty` from the same binary.
/// Read once per encoder rather than per frame.
fn raw_rgb_requested() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        matches!(
            std::env::var("GB_TERM_IMAGE").as_deref(),
            Ok("rgb") | Ok("raw")
        )
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// Capability query
// ---------------------------------------------------------------------------

/// One capability question for a terminal, and the object it refers to.
///
/// Write [`ShmQuery::seq`] to the terminal, read its reply, and pass the reply
/// to [`shm_reply_ok`]. Keep the struct alive until then: it owns the
/// shared-memory object the query names.
pub struct ShmQuery {
    /// The escape sequence to write.
    pub seq: String,
    frame: crate::shmframe::ShmFrame,
}

impl ShmQuery {
    /// Release the shared-memory object. The terminal unlinks it itself when it
    /// supports `t=s`; this covers the case where it did not.
    pub fn finish(mut self) {
        self.frame.unlink();
    }
}

/// Build a Kitty `a=q` query asking whether this terminal can read a frame out
/// of shared memory.
///
/// `a=q` is the protocol's own capability handshake: the terminal answers
/// `\x1b_Gi=<id>;OK\x1b\\` if it could have done the transfer, or
/// `\x1b_Gi=<id>;E<code>:<message>\x1b\\` if it could not, and stores nothing
/// either way. **Ask; never infer from `$TERM`** — WezTerm implements the Kitty
/// protocol but not all of it, which is precisely the trap that sent the last
/// investigation down the wrong road.
///
/// Returns `None` when this build or platform has no shared memory at all.
pub fn shm_query(id: u32) -> Option<ShmQuery> {
    // A 1×1 RGBA pixel: the smallest transfer that still exercises the medium.
    let frame = crate::shmframe::ShmFrame::new(4, |m| m.copy_from_slice(&[0, 0, 0, 0]))?;
    let mut b64 = String::new();
    base64_into(frame.name().as_bytes(), &mut b64);
    let seq = format!("\x1b_Ga=q,i={id},f=32,t=s,s=1,v=1;{b64}\x1b\\");
    Some(ShmQuery { seq, frame })
}

/// Does `reply` say the terminal can read shared memory?
///
/// Accepts the reply for image id `id` only, so a stray response to some other
/// query cannot be misread as a yes. Anything that is not an explicit `OK` — an
/// error code, a truncated reply, silence — is a no, which is the safe default:
/// guessing yes shows the user a blank rectangle.
pub fn shm_reply_ok(reply: &str, id: u32) -> bool {
    let needle = format!("_Gi={id};");
    match reply.find(&needle) {
        Some(at) => reply[at + needle.len()..].starts_with("OK"),
        None => false,
    }
}

/// Whether to place the Kitty image at the cursor (**the default**) rather than
/// through a Unicode-placeholder grid. `GB_TERM_KITTY=placeholder` restores the
/// grid.
///
/// Two reasons direct placement is the default:
///
/// 1. **Compatibility.** Unicode placeholders are a later, optional extension.
///    WezTerm implements the Kitty graphics protocol but not the placeholders,
///    so the grid renders as rows of missing-glyph boxes and no picture — which
///    is what pushed this project onto the iTerm2 protocol in the first place,
///    and cost a previous investigation real time. Direct placement is the
///    original mode and is answered `OK` by WezTerm (`render_bench --probe`).
/// 2. **Bytes.** The grid costs 4 UTF-8 bytes per covered cell *every frame*:
///    re-transmitting image data under an id deletes all of that id's
///    placements, so the grid cannot stay on screen and be reused. At 118×53
///    cells that is 25 KB per frame — after compression, far more than the
///    picture itself.
///
/// Kitty's own terminal supports both. See `docs/rendering.md` for the
/// compatibility table.
fn kitty_direct_requested() -> bool {
    !kitty_env_is("placeholder") && !kitty_env_is("placeholders")
}

/// Read `GB_TERM_KITTY`. There is no environment on `wasm32`, so it is always
/// unset there.
fn kitty_env_is(want: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("GB_TERM_KITTY").as_deref() == Ok(want)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = want;
        false
    }
}

/// Drop the alpha channel in place into `out` (used only by the raw-RGB escape
/// hatch; RGB is 25 % less to base64 than RGBA).
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
// PNG (palette-indexed when it can be, real deflate always)
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

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// The largest palette worth building. A Game Boy frame has four colours; the
/// LCD-grid upscaler tops out around twelve. Anything beyond this is not
/// Game Boy content and the truecolour path is both cheaper to reach and no
/// worse on the wire, so the palette scan bails rather than degenerating into a
/// long linear search per pixel.
const MAX_PALETTE: usize = 64;

/// Reusable PNG encoder that picks the smallest legal representation of the
/// frame it is given.
///
/// # Why palette-indexed matters here
///
/// A DMG frame has **four colours**. Sent as truecolour it is 160 × 144 × 3 =
/// 69 KB before base64; sent as a 2-bit palette image it is 40 bytes per
/// scanline — 5.8 KB — before deflate even runs, and the deflate then takes the
/// long runs down another order of magnitude. Measured on real frames this is
/// the difference between 91 KB and about 1.5 KB per frame on the wire, which is
/// the difference between 6 fps and a smooth picture in WezTerm.
///
/// Both protocols we speak accept PNG (Kitty `f=100`, iTerm2 natively), so one
/// encoder serves both.
pub struct PngEncoder {
    /// Distinct colours seen, packed `0x00RRGGBB`, in index order.
    palette: Vec<u32>,
    /// One palette index per pixel.
    index: Vec<u8>,
    /// Filtered scanlines, ready for deflate.
    raw: Vec<u8>,
    /// Candidate filtered rows (None / Sub / Up) scored per scanline.
    cand: [Vec<u8>; 3],
    /// Previous *unfiltered* scanline, which filters 1 and 2 refer back to.
    prev: Vec<u8>,
    cur: Vec<u8>,
    zlib: Vec<u8>,
    def: crate::deflate::Deflater,
    /// Reproduce the pre-compression payload: truecolour, zlib *stored* blocks.
    /// Exists so a single binary can measure before and after on the same
    /// terminal (see [`raw_rgb_requested`]).
    uncompressed: bool,
    /// Colour count of the last encode (0 when the truecolour path was used).
    pub last_colours: usize,
    /// Bit depth of the last encode.
    pub last_depth: u8,
}

impl Default for PngEncoder {
    fn default() -> Self {
        PngEncoder::new()
    }
}

impl PngEncoder {
    pub fn new() -> PngEncoder {
        PngEncoder {
            palette: Vec::new(),
            index: Vec::new(),
            raw: Vec::new(),
            cand: [Vec::new(), Vec::new(), Vec::new()],
            prev: Vec::new(),
            cur: Vec::new(),
            zlib: Vec::new(),
            def: crate::deflate::Deflater::new(),
            uncompressed: false,
            last_colours: 0,
            last_depth: 8,
        }
    }

    /// Emit the pre-compression payload (truecolour, uncompressed) so a
    /// before/after comparison can be run from one binary.
    pub fn set_uncompressed(&mut self, on: bool) {
        self.uncompressed = on;
    }

    /// Encode `rgba` (`width`×`height`, 4 bytes per pixel, alpha ignored) into
    /// `out`, which is cleared first.
    pub fn encode(&mut self, rgba: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
        debug_assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
        if !self.uncompressed && self.build_palette(rgba) {
            self.encode_indexed(width, height, out);
        } else {
            self.encode_rgb(rgba, width, height, out);
        }
    }

    /// Fill `palette`/`index` from the frame. Returns false (and leaves both in
    /// an unusable state) if the frame has more than [`MAX_PALETTE`] colours.
    fn build_palette(&mut self, rgba: &[u8]) -> bool {
        self.palette.clear();
        self.index.clear();
        self.index.reserve(rgba.len() / 4);
        // Runs of one colour are the common case, so remember the last hit and
        // skip the scan entirely for it.
        let mut last_key = u32::MAX;
        let mut last_idx = 0u8;
        for px in rgba.chunks_exact(4) {
            let key = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
            if key == last_key {
                self.index.push(last_idx);
                continue;
            }
            let idx = match self.palette.iter().position(|&c| c == key) {
                Some(i) => i,
                None => {
                    if self.palette.len() == MAX_PALETTE {
                        return false;
                    }
                    self.palette.push(key);
                    self.palette.len() - 1
                }
            };
            last_key = key;
            last_idx = idx as u8;
            self.index.push(last_idx);
        }
        true
    }

    fn encode_indexed(&mut self, width: u32, height: u32, out: &mut Vec<u8>) {
        let n = self.palette.len().max(1);
        let depth: u8 = match n {
            0..=2 => 1,
            3..=4 => 2,
            5..=16 => 4,
            _ => 8,
        };
        self.last_colours = n;
        self.last_depth = depth;

        let w = width as usize;
        let h = height as usize;
        let stride = (w * depth as usize).div_ceil(8);
        self.raw.clear();
        self.raw.reserve(h * (1 + stride));
        self.prev.clear();
        self.prev.resize(stride, 0);
        self.cur.clear();
        self.cur.resize(stride, 0);

        let per_byte = 8 / depth as usize;
        for y in 0..h {
            self.cur.iter_mut().for_each(|b| *b = 0);
            let row = &self.index[y * w..(y + 1) * w];
            for (x, &idx) in row.iter().enumerate() {
                let byte = x / per_byte;
                // PNG packs the leftmost pixel in the high-order bits.
                let shift = 8 - depth as usize * (x % per_byte + 1);
                self.cur[byte] |= idx << shift;
            }
            filter_row(&self.cur, &self.prev, 1, &mut self.cand, &mut self.raw);
            std::mem::swap(&mut self.prev, &mut self.cur);
        }

        // Palette entries are RGB triples in index order.
        let mut plte = Vec::with_capacity(n * 3);
        for &c in &self.palette {
            plte.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
        }
        if plte.is_empty() {
            plte.extend_from_slice(&[0, 0, 0]);
        }
        self.compress();
        write_png(out, width, height, depth, 3, Some(&plte), &self.zlib);
    }

    fn encode_rgb(&mut self, rgba: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
        self.last_colours = 0;
        self.last_depth = 8;
        let w = width as usize;
        let h = height as usize;
        let stride = w * 3;
        self.raw.clear();
        self.raw.reserve(h * (1 + stride));
        self.prev.clear();
        self.prev.resize(stride, 0);
        self.cur.clear();
        self.cur.resize(stride, 0);
        for y in 0..h {
            for (x, px) in rgba[y * w * 4..(y + 1) * w * 4].chunks_exact(4).enumerate() {
                self.cur[x * 3..x * 3 + 3].copy_from_slice(&px[..3]);
            }
            filter_row(&self.cur, &self.prev, 3, &mut self.cand, &mut self.raw);
            std::mem::swap(&mut self.prev, &mut self.cur);
        }
        self.compress();
        write_png(out, width, height, 8, 2, None, &self.zlib);
    }

    /// Deflate `raw` into `zlib` — or, in `uncompressed` mode, wrap it in zlib
    /// *stored* blocks, which is precisely what this module used to do.
    fn compress(&mut self) {
        if !self.uncompressed {
            self.def.zlib_into(&self.raw, &mut self.zlib);
            return;
        }
        let raw = &self.raw;
        self.zlib.clear();
        self.zlib.extend_from_slice(&[0x78, 0x01]);
        let mut pos = 0;
        while pos < raw.len() {
            let block = (raw.len() - pos).min(0xffff);
            self.zlib.push(u8::from(pos + block >= raw.len()));
            self.zlib.extend_from_slice(&(block as u16).to_le_bytes());
            self.zlib
                .extend_from_slice(&(!(block as u16)).to_le_bytes());
            self.zlib.extend_from_slice(&raw[pos..pos + block]);
            pos += block;
        }
        self.zlib
            .extend_from_slice(&crate::deflate::adler32(raw).to_be_bytes());
    }
}

/// Choose between PNG filters 0 (None), 1 (Sub) and 2 (Up) for one scanline and
/// append the winner, filter byte first.
///
/// Scoring is the standard minimum-sum-of-absolute-differences heuristic from
/// the PNG spec's own recommendations, treating filtered bytes as signed.
/// Paeth and Average are skipped: they cost noticeably more per byte and, on
/// palette-indexed Game Boy frames, win nothing that the LZ77 stage does not
/// already take.
fn filter_row(
    cur: &[u8],
    prev: &[u8],
    bpp: usize,
    cand: &mut [Vec<u8>; 3],
    out: &mut Vec<u8>,
) {
    for c in cand.iter_mut() {
        c.clear();
        c.reserve(cur.len());
    }
    for i in 0..cur.len() {
        let a = if i >= bpp { cur[i - bpp] } else { 0 };
        let b = prev[i];
        cand[0].push(cur[i]);
        cand[1].push(cur[i].wrapping_sub(a));
        cand[2].push(cur[i].wrapping_sub(b));
    }
    let score = |row: &[u8]| -> u64 {
        row.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum()
    };
    let mut best = 0usize;
    let mut best_score = score(&cand[0]);
    for (i, c) in cand.iter().enumerate().skip(1) {
        let s = score(c);
        if s < best_score {
            best_score = s;
            best = i;
        }
    }
    out.push(best as u8);
    out.extend_from_slice(&cand[best]);
}

fn write_png(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
    depth: u8,
    colour_type: u8,
    plte: Option<&[u8]>,
    idat: &[u8],
) {
    out.clear();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    let mut ihdr = [0u8; 13];
    ihdr[..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8..].copy_from_slice(&[depth, colour_type, 0, 0, 0]);
    png_chunk(out, b"IHDR", &ihdr);
    if let Some(p) = plte {
        png_chunk(out, b"PLTE", p);
    }
    png_chunk(out, b"IDAT", idat);
    png_chunk(out, b"IEND", &[]);
}

/// Encode an RGBA frame as a PNG into `out`. Convenience wrapper around
/// [`PngEncoder`] for one-shot callers; the frame loop keeps an encoder so its
/// buffers are reused.
pub fn png_into(rgba: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
    PngEncoder::new().encode(rgba, width, height, out);
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

    /// A `w`×`h` frame using exactly `colours` distinct greys, in the blocky
    /// arrangement a Game Boy actually produces (8×8 tiles), which is what the
    /// palette + deflate path is tuned for.
    fn gb_frame(w: u32, h: u32, colours: u8, seed: u32) -> Vec<u8> {
        let n = colours.max(1) as u32;
        let tiles_w = w.div_ceil(8);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let k = ((x / 8) + (y / 8) * tiles_w + seed) % n;
                let v = if n == 1 { 0 } else { (k * 255 / (n - 1)) as u8 };
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        out
    }

    #[test]
    fn png_has_valid_container_and_size() {
        let rgba = vec![0x7fu8; 4 * 3 * 4];
        let mut png = Vec::new();
        png_into(&rgba, 4, 3, &mut png);
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
        // Round-trip through the `image` crate's decoder — the same decoder
        // WezTerm uses — to prove these are real PNGs, not merely plausible
        // ones, on every path: 2-colour (1 bpp), 4-colour (2 bpp), 12-colour
        // (4 bpp) and the truecolour fallback.
        for (label, rgba, w, h) in [
            ("1bpp", gb_frame(24, 16, 2, 0), 24u32, 16u32),
            ("2bpp", gb_frame(160, 144, 4, 1), 160, 144),
            ("4bpp", gb_frame(64, 48, 12, 2), 64, 48),
            (
                "truecolour",
                (0..17 * 13 * 4)
                    .map(|i| if i % 4 == 3 { 255 } else { (i * 37) as u8 })
                    .collect::<Vec<u8>>(),
                17,
                13,
            ),
        ] {
            let mut png = Vec::new();
            png_into(&rgba, w, h, &mut png);
            let decoded = image::load_from_memory(&png)
                .unwrap_or_else(|e| panic!("{label}: not a valid PNG: {e}"));
            assert_eq!((decoded.width(), decoded.height()), (w, h), "{label}");
            let want: Vec<u8> =
                rgba.chunks_exact(4).flat_map(|p| p[..3].to_vec()).collect();
            assert_eq!(decoded.to_rgb8().into_raw(), want, "{label} pixels differ");
        }
    }

    #[test]
    fn a_four_colour_frame_costs_a_couple_of_kilobytes_not_ninety() {
        // The bug this module was rewritten for: 160×144 RGBA is 92,160 bytes
        // and base64s to ~123 KB, which is 5.5 MB/s at 60 Hz and about 6 fps in
        // WezTerm. Four colours must compress to a small fraction of that.
        let rgba = gb_frame(160, 144, 4, 3);
        let mut png = Vec::new();
        png_into(&rgba, 160, 144, &mut png);
        assert!(
            png.len() < 4096,
            "four-colour frame encoded to {} bytes",
            png.len()
        );
        // And the escape-hatch payload it replaces, for the record.
        assert!(png.len() * 20 < 160 * 144 * 3);
    }

    #[test]
    fn palette_bit_depth_follows_the_colour_count() {
        for (colours, depth) in [(2u8, 1u8), (3, 2), (4, 2), (5, 4), (16, 4)] {
            let mut enc = PngEncoder::new();
            let mut png = Vec::new();
            enc.encode(&gb_frame(64, 64, colours, 0), 64, 64, &mut png);
            assert_eq!(enc.last_colours, colours as usize, "{colours} colours");
            assert_eq!(enc.last_depth, depth, "{colours} colours");
            assert_eq!(png[24], depth, "IHDR bit depth for {colours} colours");
            assert_eq!(png[25], 3, "IHDR colour type for {colours} colours");
        }
    }

    #[test]
    fn a_high_colour_frame_falls_back_to_truecolour() {
        // Not Game Boy content, but the encoder must not corrupt or panic on it.
        let rgba: Vec<u8> = (0..64u32 * 64)
            .flat_map(|i| [(i % 256) as u8, ((i / 256) * 4) as u8, (i * 7) as u8, 255])
            .collect();
        let mut enc = PngEncoder::new();
        let mut png = Vec::new();
        enc.encode(&rgba, 64, 64, &mut png);
        assert_eq!(enc.last_colours, 0, "should have bailed to truecolour");
        assert_eq!(png[24], 8);
        assert_eq!(png[25], 2, "colour type 2 (RGB)");
    }

    /// Steady-state bytes for one *new* frame in a given rectangle: place once
    /// to settle the geometry, then place a different frame.
    fn steady_state_bytes(wire: Wire, cols: u16, rows: u16) -> usize {
        let mut t = TermImage::new(wire, 1);
        // Placeholders are the expensive Kitty mode; assert independence there.
        t.set_kitty_placeholders(true);
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
            t.set_kitty_placeholders(true);
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
    fn kitty_direct_placement_needs_no_placeholder_grid() {
        // The default. One cursor move, one transmission, no per-cell cost —
        // and it is the mode WezTerm actually implements.
        let img = gb_frame(160, 144, 4, 1);
        let mut t = TermImage::new(Wire::Kitty, 0x42);
        t.set_kitty_placeholders(false);
        let p = t.place(&img, 160, 144, CellRect::new(4, 2, 30, 14));
        let seq = p.seq;
        assert!(seq.contains("a=T,f=100"), "direct placement, PNG payload");
        assert!(!seq.contains("U=1"), "should not be a virtual placement");
        assert!(seq.contains("c=30,r=14"), "terminal-side scaling to cells");
        assert_eq!(seq.matches('\u{10EEEE}').count(), 0, "placeholders emitted");
        assert!(seq.contains("\x1b[3;5H"), "not positioned at (4,2)");
        // 30×14 cells of placeholders would be 1680 bytes on their own.
        assert!(
            p.payload_bytes < 3000,
            "{} bytes for a four-colour frame",
            p.payload_bytes
        );
    }

    #[test]
    #[cfg(all(unix, feature = "libc"))]
    fn kitty_shared_memory_sends_a_name_not_a_picture() {
        // The whole point of `t=s`: the sequence carries the *name* of a shared
        // memory object, so its size is a constant ~80 bytes no matter how big
        // the frame or the window is — and the pixels never touch the wire.
        let img = gb_frame(160, 144, 4, 5);
        let mut t = TermImage::new(Wire::Kitty, 0x77);
        t.set_kitty_shm(true);
        assert!(t.kitty_shm(), "shared memory should be available here");

        let p = t.place(&img, 160, 144, CellRect::new(2, 1, 122, 55));
        let seq = p.seq.to_string();
        assert!(seq.contains("t=s"), "not a shared-memory transmission");
        assert!(seq.contains("f=32"), "RGBA, matching our framebuffer");
        assert!(seq.contains("s=160,v=144"), "source size");
        assert!(seq.contains("c=122,r=55"), "terminal-side scaling to cells");
        assert!(!seq.contains("U=1"), "shm implies direct placement");
        assert!(
            p.payload_bytes < 160,
            "{} bytes — shm sequences must stay tiny",
            p.payload_bytes
        );

        // Read it back the way the terminal would, and check it really is our
        // frame. A sequence that names an object with the wrong bytes in it
        // would pass every assertion above and show garbage.
        let b64 = seq
            .rsplit_once(';')
            .and_then(|(_, tail)| tail.split('\x1b').next())
            .expect("payload");
        let name = String::from_utf8(base64_decode(b64)).expect("utf-8 name");
        assert!(name.starts_with("/gb-"), "unexpected object name {name}");
        let c = std::ffi::CString::new(name.clone()).unwrap();
        let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0) };
        assert!(fd >= 0, "terminal could not open {name}");
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                img.len(),
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        unsafe { libc::close(fd) };
        assert_ne!(map, libc::MAP_FAILED);
        let seen = unsafe { std::slice::from_raw_parts(map as *const u8, img.len()) };
        assert_eq!(seen, &img[..], "shared object does not hold the frame");
        unsafe { libc::munmap(map, img.len()) };

        // A bigger window must not make it bigger.
        let img2 = gb_frame(160, 144, 4, 6);
        let big = t.place(&img2, 160, 144, CellRect::new(0, 0, 280, 126));
        assert!(
            big.payload_bytes < 160,
            "{} bytes at a 4K window",
            big.payload_bytes
        );
    }

    /// Minimal base64 decoder — test-only, so the shm test can check the name
    /// the encoder actually emitted rather than one it assumed.
    #[cfg(all(unix, feature = "libc"))]
    fn base64_decode(s: &str) -> Vec<u8> {
        let val = |c: u8| -> Option<u8> {
            Some(match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return None,
            })
        };
        let bits: Vec<u8> = s.bytes().filter_map(val).collect();
        let mut out = Vec::new();
        for c in bits.chunks(4) {
            let n = c
                .iter()
                .enumerate()
                .fold(0u32, |a, (i, &v)| a | ((v as u32) << (18 - 6 * i)));
            for k in 0..c.len() - 1 {
                out.push((n >> (16 - 8 * k)) as u8);
            }
        }
        out
    }

    #[test]
    fn kitty_transmits_cell_scaling_and_positions_every_row() {
        let img = frame(160, 144, 9);
        let mut t = TermImage::new(Wire::Kitty, 0x42);
        t.set_kitty_placeholders(true);
        let p = t.place(&img, 160, 144, CellRect::new(4, 2, 30, 14));
        let seq = p.seq;
        assert!(seq.contains("a=T,U=1"), "virtual placement");
        assert!(seq.contains("s=160,v=144"), "source pixel size");
        assert!(seq.contains("c=30,r=14"), "terminal-side scaling to cells");
        assert!(seq.contains("f=100"), "PNG payload");
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
        t.set_kitty_placeholders(true);
        let p = t.place(&img, 160, 144, CellRect::new(0, 0, 5000, 5000));
        assert!(p.seq.contains(&format!("c={MAX_CELLS},r={MAX_CELLS}")));
    }

    #[test]
    fn hash_distinguishes_frames() {
        assert_ne!(fnv1a(&frame(160, 144, 1)), fnv1a(&frame(160, 144, 2)));
        assert_eq!(fnv1a(&frame(160, 144, 1)), fnv1a(&frame(160, 144, 1)));
    }
}

//! VGM — the established chiptune log format, and the reason this tool is
//! useful on day one.
//!
//! A VGM file is a timed log of *register writes to a real sound chip*, which
//! is precisely the shape of [`Track`]. So importing one is not a translation
//! with judgement in it: command `0xB3 aa dd` means "write `dd` to the Game Boy
//! register `aa` cycles-of-nothing later", and register `0x00` **is** `$FF10`
//! (VGM specification, "GameBoy DMG write"). Nothing is interpreted.
//!
//! That matters more than a bespoke syntax would: there are large public
//! archives of Game Boy VGM logs, so a `.vgm` importer makes this tool
//! immediately useful with material people already have, and it makes the
//! renderer's claim checkable against any other VGM player.
//!
//! Scope, stated honestly:
//!
//! * **`.vgz` (gzipped VGM) is not read.** It is detected and reported with
//!   the one-line fix (`gunzip`), rather than failing as a corrupt header. A
//!   DEFLATE decoder is not worth a dependency, and this crate has none.
//! * **Only the Game Boy chip is played.** A multi-chip VGM's other writes are
//!   skipped — correctly, by their documented operand lengths, so the timing of
//!   the Game Boy part is still exact — and counted, so the importer can say
//!   "1,284 writes for other chips were ignored" instead of silently rendering
//!   half a piece.
//! * VGM's sample clock is 44,100 Hz *by definition of the format*, whatever
//!   the render rate is. Waits are converted to T-cycles at that ratio.

use crate::track::{Model, Track, APU_BASE, CLOCK_HZ};

/// VGM measures time in 44.1 kHz samples regardless of anything else.
const VGM_RATE: u64 = 44_100;

/// What a VGM held, beyond the writes themselves.
#[derive(Clone, Debug, Default)]
pub struct VgmInfo {
    pub version: u32,
    /// Game Boy DMG input clock from header offset 0x80, in Hz. 0 = the file
    /// declares no Game Boy chip.
    pub gb_clock: u32,
    /// Commands for chips other than the Game Boy that were skipped.
    pub foreign_commands: u32,
    /// `total samples` from header offset 0x18, if it was set.
    pub declared_samples: u32,
}

/// Parse a VGM byte stream into a track.
pub fn parse(bytes: &[u8], model: Model) -> Result<(Track, VgmInfo), String> {
    if bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B {
        return Err(
            "this is a gzipped VGM (.vgz). Decompress it first — `gunzip -S .vgz file.vgz` \
             or `zcat file.vgz > file.vgm` — and pass the .vgm."
                .into(),
        );
    }
    if bytes.len() < 0x40 || &bytes[0..4] != b"Vgm " {
        return Err("not a VGM file (expected the magic 'Vgm ' at offset 0)".into());
    }
    let u32_at = |o: usize| -> u32 {
        if o + 4 <= bytes.len() {
            u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap())
        } else {
            0
        }
    };

    let version = u32_at(0x08);
    // Data offset is *relative to 0x34*, and is only present from v1.50; older
    // files always start their data at 0x40.
    let data_start = if version >= 0x150 {
        let rel = u32_at(0x34) as usize;
        if rel == 0 {
            0x40
        } else {
            0x34 + rel
        }
    } else {
        0x40
    };
    if data_start >= bytes.len() {
        return Err("VGM data offset points past the end of the file".into());
    }

    let mut info = VgmInfo {
        version,
        // The GB DMG clock only exists from v1.61; a 0 here just means the
        // header is older or the file declares no Game Boy.
        gb_clock: if version >= 0x161 { u32_at(0x80) } else { 0 },
        declared_samples: u32_at(0x18),
        foreign_commands: 0,
    };

    let mut track = Track::new();
    track.model = model;
    track.title = gd3_title(bytes, u32_at(0x14) as usize);

    let mut p = data_start;
    let mut samples: u64 = 0;
    let mut ended = false;
    while p < bytes.len() {
        let cmd = bytes[p];
        p += 1;
        match cmd {
            // The one command this tool exists for.
            0xB3 => {
                let (reg, val) = operands2(bytes, &mut p, "0xB3 GameBoy write")?;
                // "Register 00 equals GameBoy address FF10" — VGM spec.
                let addr = APU_BASE + reg as u16;
                if addr <= crate::track::APU_TOP {
                    track.push(samples_to_cycles(samples), addr, val);
                } else {
                    // Registers 0x30+ do not exist on the chip; a logger should
                    // never emit one, and rendering it would be inventing state.
                    info.foreign_commands += 1;
                }
            }
            0x61 => {
                let (a, b) = operands2(bytes, &mut p, "0x61 wait")?;
                samples += u16::from_le_bytes([a, b]) as u64;
            }
            0x62 => samples += 735, // one 60 Hz frame
            0x63 => samples += 882, // one 50 Hz frame
            0x66 => {
                ended = true;
                break;
            }
            0x70..=0x7F => samples += (cmd & 0x0F) as u64 + 1,
            // YM2612 DAC write + wait n: the wait still applies to us.
            0x80..=0x8F => samples += (cmd & 0x0F) as u64,
            0x67 => {
                // Data block: 0x67 0x66 tt ss ss ss ss <data>
                if p + 6 > bytes.len() {
                    return Err("truncated data block".into());
                }
                let size =
                    u32::from_le_bytes(bytes[p + 2..p + 6].try_into().unwrap()) as usize;
                p = p
                    .checked_add(6 + size)
                    .ok_or("data block length overflows the file")?;
                info.foreign_commands += 1;
            }
            0xE0 => {
                // Seek in PCM data bank — four operands, no time.
                p += 4;
                info.foreign_commands += 1;
            }
            other => {
                let n = skip_operands(other);
                if p + n > bytes.len() {
                    return Err(format!(
                        "command 0x{other:02X} at offset {} is truncated",
                        p - 1
                    ));
                }
                p += n;
                info.foreign_commands += 1;
            }
        }
    }
    if !ended && track.writes.is_empty() {
        return Err(
            "VGM contains no Game Boy writes and no end marker — wrong chip?".into(),
        );
    }

    track.sort();
    track.duration = samples_to_cycles(samples);
    // The log's own total is honoured — a VGM's `0x66` end marker is where the
    // logger decided the music stops, and stretching it would be inventing
    // audio. The one exception is a log that ends *on* its last write with no
    // trailing wait at all: there is nothing to hear, so give the final note
    // the same courtesy the score compiler does. Anything else is the caller's
    // business (`--seconds`).
    let last = track.writes.last().map(|w| w.at).unwrap_or(0);
    if track.duration <= last {
        track.duration = last + CLOCK_HZ / 10;
    }
    track.validate()?;
    Ok((track, info))
}

fn samples_to_cycles(samples: u64) -> u64 {
    (samples as u128 * CLOCK_HZ as u128 / VGM_RATE as u128) as u64
}

fn cycles_to_samples(cycles: u64) -> u64 {
    (cycles as u128 * VGM_RATE as u128 / CLOCK_HZ as u128) as u64
}

fn operands2(bytes: &[u8], p: &mut usize, what: &str) -> Result<(u8, u8), String> {
    if *p + 2 > bytes.len() {
        return Err(format!("truncated {what} at offset {}", *p));
    }
    let r = (bytes[*p], bytes[*p + 1]);
    *p += 2;
    Ok(r)
}

/// Operand length of a command this importer does not act on. The VGM
/// specification defines these ranges precisely so that a player can skip
/// chips it does not implement without losing sync — which is exactly what we
/// need, because the Game Boy part's timing depends on every wait in between.
fn skip_operands(cmd: u8) -> usize {
    match cmd {
        0x30..=0x3F => 1,
        0x40..=0x4E => 2, // 1 before v1.60; 2 is the safe modern reading
        0x4F | 0x50 => 1,
        0x51..=0x5F => 2,
        0x64 => 3,
        0x68 => 11,
        0x90 | 0x91 | 0x95 => 4,
        0x92 => 5,
        0x93 => 10,
        0x94 => 1,
        0xA0..=0xBF => 2,
        0xC0..=0xDF => 3,
        0xE1..=0xFF => 4,
        _ => 0,
    }
}

/// The GD3 tag's first field is the track name in UTF-16LE. Best effort: a
/// missing or malformed tag simply leaves the track unnamed.
fn gd3_title(bytes: &[u8], rel: usize) -> Option<String> {
    if rel == 0 {
        return None;
    }
    let at = 0x14usize.checked_add(rel)?;
    if at + 12 > bytes.len() || &bytes[at..at + 4] != b"Gd3 " {
        return None;
    }
    let mut units = Vec::new();
    let mut i = at + 12;
    while i + 1 < bytes.len() {
        let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    let s = String::from_utf16_lossy(&units);
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Write a track back out as a VGM v1.61 log, so a phrase composed here can be
/// played by any other VGM player — which is also how a reader can check this
/// renderer against a second implementation.
pub fn encode(track: &Track) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::new();
    let mut at_samples: u64 = 0;
    let mut sorted = track.writes.clone();
    sorted.sort_by_key(|w| w.at);
    for w in &sorted {
        let want = cycles_to_samples(w.at);
        push_wait(&mut data, want.saturating_sub(at_samples));
        at_samples = at_samples.max(want);
        data.push(0xB3);
        data.push((w.reg - APU_BASE) as u8);
        data.push(w.value);
    }
    let total = cycles_to_samples(track.duration);
    push_wait(&mut data, total.saturating_sub(at_samples));
    data.push(0x66);

    let mut h = vec![0u8; 0x100];
    h[0..4].copy_from_slice(b"Vgm ");
    h[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes()); // version 1.61
    h[0x18..0x1C].copy_from_slice(&(total as u32).to_le_bytes()); // total samples
    h[0x24..0x28].copy_from_slice(&0u32.to_le_bytes()); // rate: unspecified
                                                        // Data offset is relative to 0x34.
    h[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
    h[0x80..0x84].copy_from_slice(&(CLOCK_HZ as u32).to_le_bytes()); // GB DMG clock
    h.extend_from_slice(&data);
    let eof = (h.len() - 4) as u32;
    h[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
    h
}

fn push_wait(out: &mut Vec<u8>, mut n: u64) {
    while n > 0 {
        if n >= 65535 {
            out.extend_from_slice(&[0x61, 0xFF, 0xFF]);
            n -= 65535;
        } else if n > 16 {
            let w = n as u16;
            out.push(0x61);
            out.extend_from_slice(&w.to_le_bytes());
            n = 0;
        } else {
            out.push(0x70 | (n as u8 - 1));
            n = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A VGM built here, so nothing copyrighted is involved: power on, one
    /// note, one second.
    fn hand_built() -> Vec<u8> {
        let mut t = Track::new();
        t.push(0, 0xFF26, 0x80);
        t.push(0, 0xFF24, 0x77);
        t.push(0, 0xFF25, 0xFF);
        t.push(0, 0xFF11, 0x80);
        t.push(0, 0xFF12, 0xF0);
        t.push(0, 0xFF13, 0xD6);
        t.push(0, 0xFF14, 0x86);
        t.push(CLOCK_HZ / 2, 0xFF12, 0x00);
        t.duration = CLOCK_HZ;
        encode(&t)
    }

    #[test]
    fn a_vgm_round_trips_through_this_crate() {
        let bytes = hand_built();
        let (t, info) = parse(&bytes, Model::Dmg).unwrap();
        assert_eq!(info.version, 0x161);
        assert_eq!(info.gb_clock, CLOCK_HZ as u32);
        assert_eq!(info.foreign_commands, 0);
        assert_eq!(t.writes.len(), 8);
        assert_eq!(t.writes[0].reg, 0xFF26);
        assert_eq!(t.writes[0].value, 0x80);
        // The note-off lands within one 44.1 kHz sample of half a second —
        // VGM's clock is coarser than the APU's, and that is the format's
        // resolution, not a rounding bug of ours.
        let off = t.writes.last().unwrap();
        assert_eq!(off.reg, 0xFF12);
        let err = (off.at as i64 - (CLOCK_HZ / 2) as i64).abs();
        assert!(
            err < (CLOCK_HZ / VGM_RATE) as i64 + 1,
            "off by {err} cycles"
        );
    }

    #[test]
    fn register_zero_is_ff10() {
        // The single fact the whole importer rests on.
        let bytes = {
            let mut h = vec![0u8; 0x100];
            h[0..4].copy_from_slice(b"Vgm ");
            h[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
            h[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
            h.extend_from_slice(&[0xB3, 0x00, 0x12, 0xB3, 0x16, 0x80, 0x66]);
            h
        };
        let (t, _) = parse(&bytes, Model::Dmg).unwrap();
        assert_eq!(t.writes[0].reg, 0xFF10);
        assert_eq!(t.writes[1].reg, 0xFF26, "register 0x16 is NR52");
    }

    #[test]
    fn another_chips_commands_are_skipped_without_losing_time() {
        let mut h = vec![0u8; 0x100];
        h[0..4].copy_from_slice(b"Vgm ");
        h[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
        h[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
        h.extend_from_slice(&[
            0x52, 0x28, 0x00, // YM2612 port 0 write — two operands, skipped
            0x62, // wait one 60 Hz frame
            0xB3, 0x16, 0x80, // our NR52 write, *after* that wait
            0x66,
        ]);
        let (t, info) = parse(&h, Model::Dmg).unwrap();
        assert_eq!(info.foreign_commands, 1);
        assert_eq!(t.writes.len(), 1);
        assert_eq!(t.writes[0].at, samples_to_cycles(735));
    }

    #[test]
    fn a_vgz_is_named_not_mangled() {
        let e = parse(&[0x1F, 0x8B, 0x08, 0x00], Model::Dmg).unwrap_err();
        assert!(e.contains("gunzip"), "{e}");
    }

    #[test]
    fn waits_encode_compactly_and_exactly() {
        for n in [1u64, 16, 17, 735, 65534, 65535, 70000] {
            let mut v = Vec::new();
            push_wait(&mut v, n);
            // Decode it back through the parser's own arithmetic.
            let mut p = 0usize;
            let mut got = 0u64;
            while p < v.len() {
                let c = v[p];
                p += 1;
                match c {
                    0x61 => {
                        got += u16::from_le_bytes([v[p], v[p + 1]]) as u64;
                        p += 2;
                    }
                    0x70..=0x7F => got += (c & 0x0F) as u64 + 1,
                    _ => panic!("unexpected wait encoding 0x{c:02X}"),
                }
            }
            assert_eq!(got, n);
        }
    }
}

//! 16-bit stereo RIFF/PCM, hand-rolled — no dependency, and none wanted.
//!
//! This is the encoder that has lived privately inside
//! `testharness/audiocapture.rs` since the audio harness was written; it is
//! here so there is exactly **one** of it. The bytes are unchanged on purpose:
//! `testharness/audio_baseline.txt` pins an FNV hash of the interleaved PCM,
//! so any drift in [`to_i16`] or [`interleave_i16`] would move a baseline that
//! has nothing to do with music rendering.

use std::io;
use std::path::Path;

/// Sample conversion. Clamped, then rounded — not truncated: truncation biases
/// every sample towards zero and shows up as a measurable DC-free dullness.
pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Little-endian interleaved L,R 16-bit PCM — the payload of the `data` chunk,
/// and what the audio baseline hashes.
pub fn interleave_i16(samples: &[(f32, f32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for &(l, r) in samples {
        out.extend_from_slice(&to_i16(l).to_le_bytes());
        out.extend_from_slice(&to_i16(r).to_le_bytes());
    }
    out
}

/// A complete `.wav` file in memory.
pub fn encode(samples: &[(f32, f32)], rate: u32) -> Vec<u8> {
    encode_pcm(&interleave_i16(samples), rate)
}

/// Wrap already-interleaved PCM in the 44-byte canonical header.
pub fn encode_pcm(pcm: &[u8], rate: u32) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // stereo
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
    w.extend_from_slice(&4u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    w
}

/// Write a `.wav`, creating the parent directory. Returns the error rather
/// than panicking — this is library code, not a test harness.
pub fn write(path: &Path, samples: &[(f32, f32)], rate: u32) -> io::Result<()> {
    write_pcm(path, &interleave_i16(samples), rate)
}

pub fn write_pcm(path: &Path, pcm: &[u8], rate: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, encode_pcm(pcm, rate))
}

/// FNV-1a, the hash `testharness/audio_baseline.txt` records. Here so a caller
/// can pin a render without inventing a second one.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header, byte for byte, so nothing can quietly drift away from the
    /// bytes `testharness/audiocapture.rs` has always produced.
    #[test]
    fn the_header_is_the_canonical_44_bytes() {
        let w = encode(&[(0.0, 0.0); 2], 44_100);
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(&w[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(w[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 44_100);
        assert_eq!(
            u32::from_le_bytes(w[28..32].try_into().unwrap()),
            44_100 * 4
        );
        assert_eq!(u16::from_le_bytes(w[32..34].try_into().unwrap()), 4);
        assert_eq!(u16::from_le_bytes(w[34..36].try_into().unwrap()), 16);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
    }

    #[test]
    fn samples_clamp_and_round() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
        assert_eq!(to_i16(9.0), 32767);
        assert_eq!(to_i16(-9.0), -32767);
        // Rounds, does not truncate.
        assert_eq!(to_i16(0.5 / 32767.0), 1);
    }

    #[test]
    fn interleaving_is_left_then_right() {
        let pcm = interleave_i16(&[(1.0, -1.0)]);
        assert_eq!(pcm, vec![0xFF, 0x7F, 0x01, 0x80]);
    }
}

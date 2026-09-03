//! A 16-bit PCM WAV writer — the canonical 44-byte RIFF/WAVE header (a public
//! file format, not anyone's creative work) plus interleaved samples. Kept
//! here, hand-rolled, so this crate has no dependency for the one file
//! format it needs to produce — the same call `terminalgb-chiptune::wav` and
//! ShowReel's own `music.rs` each make independently, for the same reason.

/// Convert a float sample to `i16`, clamping first and rounding rather than
/// truncating — truncation biases every sample toward zero, which is
/// measurable as a quiet, dull loss of level across a whole render.
pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Interleave mono `f32` samples into stereo (duplicated to both channels)
/// 16-bit little-endian PCM.
pub fn interleave_mono_to_stereo_i16(mono: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(mono.len() * 4);
    for &s in mono {
        let v = to_i16(s).to_le_bytes();
        out.extend_from_slice(&v);
        out.extend_from_slice(&v);
    }
    out
}

/// Wrap already-interleaved 16-bit PCM in a canonical stereo WAV header.
pub fn encode_pcm_stereo(pcm: &[u8], sample_rate_hz: u32) -> Vec<u8> {
    let channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let block_align = channels as u32 * (bits_per_sample as u32 / 8);
    let byte_rate = sample_rate_hz * block_align;
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    w.extend_from_slice(&1u16.to_le_bytes()); // audio format 1 = PCM
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&sample_rate_hz.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&(block_align as u16).to_le_bytes());
    w.extend_from_slice(&bits_per_sample.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    w
}

/// A mono buffer as a complete stereo WAV file in memory (both channels
/// carry the same signal — this crate's pieces are centred, the way
/// ShowReel's chiptune reference is).
pub fn mono_to_wav_bytes(mono: &[f32], sample_rate_hz: u32) -> Vec<u8> {
    encode_pcm_stereo(&interleave_mono_to_stereo_i16(mono), sample_rate_hz)
}

/// Write a mono buffer to `path` as a stereo WAV, creating the parent
/// directory if needed.
pub fn write_mono(
    path: &std::path::Path,
    mono: &[f32],
    sample_rate_hz: u32,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, mono_to_wav_bytes(mono, sample_rate_hz))
}

/// Read a canonical PCM WAV back into interleaved `f32` samples plus its
/// channel count and sample rate — enough for [`crate::compare`] to judge two
/// renders, and independent of whatever wrote the file (this reads any
/// standard-form 8/16/24/32-bit integer or 32-bit float PCM WAV, not only
/// ones this module wrote).
pub fn read(path: &std::path::Path) -> std::io::Result<(Vec<f32>, u16, u32)> {
    let bytes = std::fs::read(path)?;
    parse(&bytes).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "not a canonical PCM WAV")
    })
}

fn parse(bytes: &[u8]) -> Option<(Vec<f32>, u16, u32)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut audio_format = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                audio_format = u16::from_le_bytes(body[0..2].try_into().ok()?);
                channels = u16::from_le_bytes(body[2..4].try_into().ok()?);
                sample_rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
                bits_per_sample = u16::from_le_bytes(body[14..16].try_into().ok()?);
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are word-aligned: a chunk of odd length is followed by one
        // pad byte that is not counted in its own size.
        pos = body_start + size + (size & 1);
    }
    let data = data?;
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    let samples: Vec<f32> = match (audio_format, bits_per_sample) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        _ => return None,
    };
    Some((samples, channels, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_temp_file() {
        let dir = std::env::temp_dir()
            .join(format!("loom-synth-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let mono: Vec<f32> = (0..100)
            .map(|i| (i as f32 / 50.0 - 1.0).clamp(-1.0, 1.0))
            .collect();
        write_mono(&path, &mono, 8000).unwrap();
        let (got, channels, rate) = read(&path).unwrap();
        assert_eq!(channels, 2);
        assert_eq!(rate, 8000);
        assert_eq!(got.len(), mono.len() * 2);
        // Left and right should be identical (centred mono duplicated).
        for i in 0..mono.len() {
            assert_eq!(got[i * 2], got[i * 2 + 1]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn header_fields_are_correct() {
        let bytes = mono_to_wav_bytes(&[0.0, 0.0], 44_100);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2); // channels
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            44_100
        );
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16); // bits/sample
    }

    #[test]
    fn samples_clamp_and_round() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
        assert_eq!(to_i16(5.0), 32767);
        assert_eq!(to_i16(-5.0), -32767);
    }
}

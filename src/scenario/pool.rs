//! An entry pool of save states, stored once rather than 3,000 times.
//!
//! # The measurement that put this here
//!
//! A corpus directory's bytes are not where anyone expects. Measured on a real
//! 720-attempt Mt Moon film pass (`~/pokemon-run/mtmoon-film`,
//! 143 MB):
//!
//! | | files | on disk |
//! |---|---:|---:|
//! | exit save states (`.gbss`) | 720 | **126.08 MB** |
//! | recordings (`.json`) | 720 | 10.27 MB |
//! | position tracks (`.csv`) | 720 | 9.07 MB |
//!
//! **88 % of it is the save-state pool**, so the argument that position
//! recording is too expensive to leave on was aimed at the wrong file all
//! along. And a pool compresses far better than a general compressor can see:
//! every state in it is the *same machine* a few thousand frames apart, so
//! **93.2 % of the 175,114 bytes are byte-identical across all 720 states**.
//! `gzip -9` on each file separately only finds 5.4x, because a 32 KiB window
//! cannot see across a 175 KB file to the one beside it.
//!
//! Referencing one state and storing the runs that differ gives **33.4x with
//! no compression at all** — 21.01 MB of one link's 120 exits becomes 0.63 MB
//! — and 78.3x with `gzip` on top of the deltas. This module implements the
//! uncompressed form, which needs no inflater and is what the numbers above
//! were measured with.
//!
//! # The format — `GBPL`, version 1
//!
//! ```text
//! magic     "GBPL"     4 bytes
//! version   u8         1
//! flags     u8         0; reserved, refused when non-zero
//! count     u32 LE     members that follow
//! ref_len   u32 LE     the reference member's length
//! reference bytes      ref_len of them — member 0, whole
//! members...           count of them, member 0 included and empty
//! ```
//!
//! Each member:
//!
//! ```text
//! name_len  u8         bytes of name (a file stem, so a caller can put the
//!                      directory back exactly as it was)
//! name      bytes
//! len       varint     this member's own length
//! runs      varint     differing runs against the reference
//! runs:     at         varint  offset into the member
//!           n          varint  bytes that follow
//!           bytes      n of them
//! ```
//!
//! A member shorter or longer than the reference is expressible: the
//! reference is truncated or zero-extended to `len` before the runs land, and
//! bytes past the reference are simply one long run. Nothing here assumes the
//! members are the same size, though in practice a pool's states always are —
//! `savestate::FORMAT_VERSION` fixes the layout and the cartridge fixes the RAM.
//!
//! # What this is not
//!
//! It is not a general archive and it must not become one. Two properties are
//! load-bearing and both come from the members being *the same machine*: the
//! reference is a real member rather than a synthesised one, so a pool always
//! round-trips even if the delta coder is wrong about everything else; and
//! order is preserved, because a seed takes pool slot `seed % len` and a pool
//! that reorders itself silently re-points every attempt in a corpus.

/// `GBPL`.
pub const MAGIC: [u8; 4] = *b"GBPL";
/// The version this build writes and the only one it reads.
pub const VERSION: u8 = 1;

/// One save state and the name it had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The file stem it came from — `00042` for `00042.gbss`. Kept so
    /// unpacking puts a directory back exactly as it was, which matters
    /// because a seed's entry is `pool[seed % len]` over the *sorted* names.
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A pool of same-shaped save states, delta-encoded against the first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pool {
    members: Vec<Member>,
}

impl Pool {
    pub fn new(members: Vec<Member>) -> Pool {
        Pool { members }
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The whole pool as `GBPL` bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(0);
        out.extend_from_slice(&(self.members.len() as u32).to_le_bytes());
        let empty: Vec<u8> = Vec::new();
        let reference = self.members.first().map(|m| &m.bytes).unwrap_or(&empty);
        out.extend_from_slice(&(reference.len() as u32).to_le_bytes());
        out.extend_from_slice(reference);
        for m in &self.members {
            let name = m.name.as_bytes();
            out.push(name.len().min(255) as u8);
            out.extend_from_slice(&name[..name.len().min(255)]);
            put_varint(&mut out, m.bytes.len() as u64);
            let runs = diff_runs(reference, &m.bytes);
            put_varint(&mut out, runs.len() as u64);
            for (at, n) in runs {
                put_varint(&mut out, at as u64);
                put_varint(&mut out, n as u64);
                out.extend_from_slice(&m.bytes[at..at + n]);
            }
        }
        out
    }

    /// Read a `GBPL` file back. Every failure names what was wrong.
    pub fn decode(b: &[u8]) -> Result<Pool, String> {
        if b.len() < 14 {
            return Err(format!("too short to be a pool: {} byte(s)", b.len()));
        }
        if b[..4] != MAGIC {
            return Err("not a GBPL pool (bad magic)".into());
        }
        if b[4] != VERSION {
            return Err(format!(
                "pool version {} — this build reads {VERSION}",
                b[4]
            ));
        }
        if b[5] != 0 {
            return Err(format!("pool flags {:#04x} are not understood", b[5]));
        }
        let count = u32::from_le_bytes([b[6], b[7], b[8], b[9]]) as usize;
        let ref_len = u32::from_le_bytes([b[10], b[11], b[12], b[13]]) as usize;
        let mut at = 14;
        if b.len() < at + ref_len {
            return Err("the reference member is truncated".into());
        }
        let reference = &b[at..at + ref_len];
        at += ref_len;
        let mut members = Vec::with_capacity(count);
        for i in 0..count {
            if at >= b.len() {
                return Err(format!("ran out of bytes at member {i} of {count}"));
            }
            let nl = b[at] as usize;
            at += 1;
            if b.len() < at + nl {
                return Err(format!("member {i}: truncated name"));
            }
            let name = String::from_utf8_lossy(&b[at..at + nl]).into_owned();
            at += nl;
            let (len, n) = get_varint(&b[at..])
                .ok_or_else(|| format!("member {i}: truncated length"))?;
            at += n;
            let len = len as usize;
            let (runs, n) = get_varint(&b[at..])
                .ok_or_else(|| format!("member {i}: truncated run count"))?;
            at += n;
            let mut bytes = vec![0u8; len];
            let take = len.min(reference.len());
            bytes[..take].copy_from_slice(&reference[..take]);
            for _ in 0..runs {
                let (off, n) = get_varint(&b[at..])
                    .ok_or_else(|| format!("member {i}: truncated run offset"))?;
                at += n;
                let (rl, n) = get_varint(&b[at..])
                    .ok_or_else(|| format!("member {i}: truncated run length"))?;
                at += n;
                let (off, rl) = (off as usize, rl as usize);
                if off + rl > len || at + rl > b.len() {
                    return Err(format!("member {i}: a run runs past the end"));
                }
                bytes[off..off + rl].copy_from_slice(&b[at..at + rl]);
                at += rl;
            }
            members.push(Member { name, bytes });
        }
        Ok(Pool { members })
    }
}

/// The runs of `b` that differ from `a`, as `(offset, length)`.
///
/// Bytes past the end of the reference are a difference by definition, so a
/// longer member gets one run covering its tail.
fn diff_runs(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let same = i < a.len() && a[i] == b[i];
        if same {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && !(i < a.len() && a[i] == b[i]) {
            i += 1;
        }
        runs.push((start, i - start));
    }
    runs
}

fn put_varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let x = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(x);
            return;
        }
        out.push(x | 0x80);
    }
}

fn get_varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut n = 0u64;
    for (i, byte) in b.iter().take(10).enumerate() {
        n |= ((byte & 0x7F) as u64) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((n, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool of states shaped like a real one.
    ///
    /// **The shape is measured, not invented**: a real exit pool's 175,114-byte
    /// states differ from each other in 11,823 offsets, and those offsets fall
    /// in about 300 contiguous runs of ~7 bytes rather than being scattered —
    /// they are the handful of WRAM regions the game moved. A fixture that
    /// scatters the same number of *isolated* bytes measures a different
    /// problem (4,000 run headers instead of 300) and reports a ratio a real
    /// pool never has.
    fn realistic(n: usize) -> Pool {
        let base: Vec<u8> = (0..175_114u32)
            .map(|i| (i.wrapping_mul(37) >> 3) as u8)
            .collect();
        Pool::new(
            (0..n)
                .map(|k| {
                    let mut b = base.clone();
                    for r in 0..300usize {
                        let at = (r * 577 + k * 31) % (b.len() - 64);
                        for j in 0..7usize {
                            b[at + j] = (k as u8).wrapping_add((r + j) as u8);
                        }
                    }
                    Member {
                        name: format!("{k:05}"),
                        bytes: b,
                    }
                })
                .collect(),
        )
    }

    #[test]
    fn a_pool_round_trips_through_its_own_bytes() {
        let p = realistic(12);
        assert_eq!(Pool::decode(&p.encode()).expect("decode"), p);
    }

    /// The whole reason it exists. Teeth: store the members whole and this
    /// goes red.
    ///
    /// The bar is 12x rather than the 33.4x a real pool measures, because a
    /// real pool has 120 members to amortise the reference over and this
    /// fixture has 40. The real figure is in the module docs and was measured
    /// on real states, not on this.
    #[test]
    fn a_pool_of_near_identical_states_is_an_order_of_magnitude_smaller() {
        let p = realistic(40);
        let raw: usize = p.members().iter().map(|m| m.bytes.len()).sum();
        let n = p.encode().len();
        assert!(
            raw / n >= 12,
            "{raw} bytes of states packed to {n} — only {:.1}x",
            raw as f64 / n as f64
        );
    }

    /// **Order is load-bearing**: a seed takes slot `seed % len`, so a pool
    /// that reordered itself would silently re-point every attempt in a corpus.
    #[test]
    fn names_and_order_survive_a_round_trip() {
        let p = realistic(6);
        let back = Pool::decode(&p.encode()).unwrap();
        assert_eq!(
            back.members()
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>(),
            vec!["00000", "00001", "00002", "00003", "00004", "00005"]
        );
    }

    #[test]
    fn members_of_different_lengths_are_expressible() {
        let p = Pool::new(vec![
            Member {
                name: "a".into(),
                bytes: vec![1, 2, 3, 4, 5],
            },
            Member {
                name: "b".into(),
                bytes: vec![1, 2, 9],
            },
            Member {
                name: "c".into(),
                bytes: vec![1, 2, 3, 4, 5, 6, 7],
            },
        ]);
        assert_eq!(Pool::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn an_empty_pool_is_a_valid_file() {
        let p = Pool::new(Vec::new());
        assert!(Pool::decode(&p.encode()).unwrap().is_empty());
    }

    #[test]
    fn a_damaged_pool_is_refused_rather_than_decoded_wrong() {
        let good = realistic(3).encode();
        assert!(Pool::decode(&good[..10]).is_err(), "short");
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(Pool::decode(&bad).is_err(), "bad magic");
        let mut bad = good.clone();
        bad[4] = 7;
        assert!(Pool::decode(&bad).is_err(), "wrong version");
        assert!(Pool::decode(&good[..good.len() - 20]).is_err(), "truncated");
    }
}

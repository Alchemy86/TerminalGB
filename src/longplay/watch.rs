//! Declarative memory watches — **for reporting only**.
//!
//! # The separation this module exists to keep
//!
//! A watch samples a named address out of the running machine and puts the
//! number in the journal and the report. It is **never** shown to the agent.
//!
//! That is not tidiness, it is what makes an evaluation mean anything. An agent
//! told "the opponent has 4 HP left" is not solving the problem the screen
//! poses; it is reading the answer out of RAM. If watches leaked into the
//! snapshot, every episode success rate this harness ever produced would be
//! uninterpretable, and — worse — the leak would be invisible in the results.
//!
//! So the two paths are structurally separate and stay that way:
//!
//! | path | reads | goes to |
//! |---|---|---|
//! | the agent's | its own `Gen1Observer` + the screen, inside the plugin | the model |
//! | a watch | this module, inside the **host** | `events.ndjson`, `episodes.ndjson`, the report |
//!
//! Nothing in `src/longplay` ever hands a [`Sample`] to a plugin, and
//! `testharness/longplay.rs` asserts it by grepping every request the driver
//! was actually given for the watch's name and value.
//!
//! # Addresses are reused, not rediscovered
//!
//! [`Watch::named`] resolves a handful of human names to the addresses
//! [`terminalgb_plugin_trainer::gen1`] has already derived and verified. A raw
//! `0xXXXX` is accepted too, because a person watching a hack or a different
//! game needs one — but the named form is what a shared episode file should
//! use, since a name survives a re-derivation and a number does not.

use terminalgb_plugin_agent::json;
use terminalgb_plugin_api::Machine;
use terminalgb_plugin_trainer::gen1::addr as tr;

/// One address to sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub name: String,
    pub addr: u16,
    /// 1, 2 or 4 bytes.
    pub len: u8,
    /// Gen 1 multi-byte fields are big-endian; that is the default here for
    /// exactly that reason.
    pub big_endian: bool,
}

/// The built-in names, each pointing at an address another crate derived.
///
/// Kept short on purpose. Anything a run reports on *routinely* — position,
/// badges, party, money, the Pokédex — is in [`super::progress::Progress`]
/// already and needs no watch; these are the extras an episode about a
/// specific problem wants.
pub const NAMED: &[(&str, u16, u8, bool)] = &[
    ("in_battle", tr::IS_IN_BATTLE, 1, false),
    ("battle_type", tr::BATTLE_TYPE, 1, false),
    ("cur_opponent", tr::CUR_OPPONENT, 1, false),
    ("enemy_species", tr::ENEMY_MON, 1, false),
    // `enemy_mon::HP` is offset 1 into `wEnemyMon`, big-endian.
    ("enemy_hp", tr::ENEMY_MON + 1, 2, true),
    ("enemy_level", tr::ENEMY_MON + 14, 1, false),
    ("trainer_class", tr::TRAINER_CLASS, 1, false),
    // `party_struct::HP` is offset 1 into `wPartyMon1`, big-endian.
    ("lead_hp", tr::PARTY_MONS + 1, 2, true),
    ("lead_max_hp", tr::PARTY_MONS + 34, 2, true),
    ("lead_level", tr::PARTY_MONS + 33, 1, false),
    ("lead_status", tr::PARTY_MONS + 4, 1, false),
];

impl Watch {
    /// Resolve a built-in name.
    pub fn named(name: &str) -> Option<Watch> {
        NAMED
            .iter()
            .find(|(n, ..)| *n == name)
            .map(|(n, addr, len, be)| Watch {
                name: (*n).to_string(),
                addr: *addr,
                len: *len,
                big_endian: *be,
            })
    }

    /// Parse one watch out of an episode file's `watch` array.
    ///
    /// Two forms: `"enemy_hp"` (a built-in) or
    /// `{"name":"flag","addr":"0xD747","len":1}`.
    pub fn from_json(v: &json::Json) -> Result<Watch, String> {
        if let Some(s) = v.as_str() {
            return Watch::named(s).ok_or_else(|| {
                format!(
                    "unknown watch {s:?} — known names are {}",
                    NAMED
                        .iter()
                        .map(|(n, ..)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        }
        let name = v
            .str_field("name")
            .ok_or("a watch needs a name")?
            .to_string();
        if name.is_empty() {
            return Err("a watch's name may not be empty".into());
        }
        let addr = match v.get("addr") {
            Some(a) => parse_addr(a)?,
            None => {
                return Watch::named(&name).ok_or(format!(
                    "watch {name:?} has no addr and is not a known name"
                ))
            }
        };
        let len = v.u64_field("len").unwrap_or(1) as u8;
        if !matches!(len, 1 | 2 | 4) {
            return Err(format!("watch {name:?}: len must be 1, 2 or 4, not {len}"));
        }
        Ok(Watch {
            name,
            addr,
            len,
            big_endian: v
                .get("big_endian")
                .and_then(|b| b.as_bool())
                .unwrap_or(true),
        })
    }

    pub fn to_json(&self, w: &mut json::Writer) {
        w.begin_obj();
        w.str_field("name", &self.name);
        w.str_field("addr", &format!("{:#06X}", self.addr));
        w.num_field("len", self.len as f64);
        w.bool_field("big_endian", self.big_endian);
        w.end_obj();
    }

    fn read(&self, m: &mut Machine) -> u32 {
        let mut bytes = [0u8; 4];
        m.peek_into(self.addr, &mut bytes[..self.len as usize]);
        let mut v = 0u32;
        if self.big_endian {
            for b in &bytes[..self.len as usize] {
                v = (v << 8) | *b as u32;
            }
        } else {
            for b in bytes[..self.len as usize].iter().rev() {
                v = (v << 8) | *b as u32;
            }
        }
        v
    }
}

/// `"0xD747"`, `"$D747"` or `53575`.
fn parse_addr(v: &json::Json) -> Result<u16, String> {
    if let Some(n) = v.as_u64() {
        return u16::try_from(n).map_err(|_| format!("{n} is not a 16-bit address"));
    }
    let s = v
        .as_str()
        .ok_or("addr must be a number or a string")?
        .trim();
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .or_else(|| s.strip_prefix('$'));
    match hex {
        Some(h) => {
            u16::from_str_radix(h, 16).map_err(|_| format!("{s:?} is not an address"))
        }
        None => s.parse().map_err(|_| format!("{s:?} is not an address")),
    }
}

/// The watch list of one run or episode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watches(pub Vec<Watch>);

impl Watches {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Read every watch. ~1 bus read per byte watched.
    pub fn read(&self, m: &mut Machine) -> Sample {
        Sample(self.0.iter().map(|w| (w.name.clone(), w.read(m))).collect())
    }

    pub fn get(&self, name: &str) -> Option<&Watch> {
        self.0.iter().find(|w| w.name == name)
    }
}

/// One reading of every watch. **Reporting only** — see the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sample(pub Vec<(String, u32)>);

impl Sample {
    pub fn get(&self, name: &str) -> Option<u32> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Have any of the watched values moved?
    pub fn differs(&self, other: &Sample) -> bool {
        self != other
    }

    pub fn write_json(&self, w: &mut json::Writer) {
        w.begin_obj();
        for (name, value) in &self.0 {
            w.num_field(name, *value as f64);
        }
        w.end_obj();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_name_resolves_to_an_address_another_crate_derived() {
        let w = Watch::named("enemy_hp").expect("known");
        assert_eq!(w.addr, tr::ENEMY_MON + 1);
        assert_eq!(w.len, 2);
        assert!(w.big_endian, "every multi-byte Gen 1 field is big-endian");
        assert!(Watch::named("no_such_thing").is_none());
    }

    #[test]
    fn an_unknown_name_is_refused_with_the_list_of_known_ones() {
        let v = json::parse("\"nonsense\"").unwrap();
        let e = Watch::from_json(&v).unwrap_err();
        assert!(e.contains("enemy_hp"), "{e}");
    }

    #[test]
    fn a_raw_address_is_accepted_in_every_spelling_a_person_writes() {
        for spelling in ["\"0xD747\"", "\"$D747\"", "55111"] {
            let text = format!(r#"{{"name":"flag","addr":{spelling},"len":1}}"#);
            let v = json::parse(&text).unwrap();
            let w = Watch::from_json(&v).expect(spelling);
            assert_eq!(w.addr, 0xD747, "{spelling}");
        }
        // A bare "D747" is ambiguous — decimal or hex? — so it is refused
        // rather than guessed at. An address read wrong is a watch that
        // reports a plausible number about the wrong byte for ever.
        let v = json::parse(r#"{"name":"flag","addr":"D747","len":1}"#).unwrap();
        assert!(Watch::from_json(&v).is_err());
    }

    #[test]
    fn a_silly_length_is_refused_rather_than_read_as_garbage() {
        let v = json::parse(r#"{"name":"x","addr":"0xC000","len":3}"#).unwrap();
        assert!(Watch::from_json(&v).unwrap_err().contains("1, 2 or 4"));
    }

    #[test]
    fn a_watch_round_trips_through_its_own_json() {
        let w = Watch {
            name: "flag".into(),
            addr: 0xD747,
            len: 2,
            big_endian: false,
        };
        let mut out = json::Writer::new();
        w.to_json(&mut out);
        let back = Watch::from_json(&json::parse(&out.finish()).unwrap()).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn a_sample_is_a_plain_name_to_number_map() {
        let s = Sample(vec![("hp".into(), 21), ("lvl".into(), 7)]);
        assert_eq!(s.get("hp"), Some(21));
        assert_eq!(s.get("nope"), None);
        let mut w = json::Writer::new();
        s.write_json(&mut w);
        let text = w.finish();
        assert!(text.contains("\"hp\":21"), "{text}");
        assert!(s.differs(&Sample::default()));
    }
}

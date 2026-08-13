//! Where the Pokémon comes from — deliberately separate from how it is traded.
//!
//! [`protocol::LinkPartner`](super::protocol::LinkPartner) knows the link cable
//! and nothing else; it is handed a finished [`Party`]. Everything about
//! *choosing* what to offer lives behind [`PokemonSource`].
//!
//! That split is the point. A virtual link partner is already a **bridge**: a
//! Pokémon leaves the running game through a genuine trade and something arrives
//! in its place, and nothing in the protocol cares whether that something came
//! from a text file, another emulator, a converted Generation 3 save, or a
//! remote player. Today there is one implementation, [`TradeSpec`], reading a
//! small `key = value` file. A network partner, a save-file reader or a
//! generation converter are additional implementations of this one trait, not
//! changes to the protocol.
//!
//! # File format
//!
//! The same hand-rolled `key = value` shape the terminal frontend's config uses
//! — no TOML dependency, `#` starts a comment, values may be bare or quoted. A
//! `species` line starts a new Pokémon, so up to six can be listed.
//!
//! ```text
//! trainer  = ORACLE        # the partner's trainer name, shown during the trade
//!
//! species  = Mew           # name, "#151" for a Pokédex number, or an index
//! level    = 5
//! nickname = MEW
//! ot_name  = GF
//! ot_id    = 22796
//! moves    = Pound
//! dvs      = 15, 15, 15, 15    # attack, defense, speed, special (0-15)
//! ```

use super::party::{self, Party};
use super::romdata::{RomError, RomTables};

/// A source of Pokémon for the link partner to offer.
///
/// The one seam that makes remote trading, cross-generation transfer and
/// event distribution the same feature.
pub trait PokemonSource {
    /// One line describing where these Pokémon come from, for the status bar.
    fn describe(&self) -> String;
    /// Build the party to offer. Takes the cartridge's own tables so species,
    /// stats and moves are resolved against the game actually running.
    fn party(&self, tables: &RomTables) -> Result<Party, SourceError>;
}

#[derive(Debug)]
pub enum SourceError {
    Rom(RomError),
    Party(party::PartyError),
    Parse { line: usize, message: String },
    Io(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Rom(e) => write!(f, "{e}"),
            SourceError::Party(e) => write!(f, "{e}"),
            SourceError::Parse { line, message } => {
                write!(f, "trade file line {line}: {message}")
            }
            SourceError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<RomError> for SourceError {
    fn from(e: RomError) -> Self {
        SourceError::Rom(e)
    }
}
impl From<party::PartyError> for SourceError {
    fn from(e: party::PartyError) -> Self {
        SourceError::Party(e)
    }
}

/// One requested Pokémon, before it is resolved against a cartridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonSpec {
    pub species: String,
    pub level: u8,
    pub nickname: String,
    pub ot_name: String,
    pub ot_id: u16,
    pub moves: Vec<String>,
    /// Attack, Defense, Speed, Special. The HP DV is derived from these.
    pub dvs: (u8, u8, u8, u8),
}

impl Default for MonSpec {
    fn default() -> Self {
        MonSpec {
            species: String::new(),
            level: 5,
            nickname: String::new(),
            ot_name: "GAMEBOY".into(),
            ot_id: 0x1337,
            moves: Vec::new(),
            dvs: (15, 15, 15, 15),
        }
    }
}

/// A parsed trade file: a trainer name and up to six Pokémon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeSpec {
    pub trainer: String,
    pub mons: Vec<MonSpec>,
    /// Where this came from, for the status line.
    pub origin: String,
}

impl Default for TradeSpec {
    /// The out-of-the-box offer: the level 5 Mew that the real distribution
    /// events handed out. Chosen because it exercises the two things most
    /// likely to be wrong — Mew's base stats live outside the main table in
    /// Red/Blue, and a level 5 legendary is obviously not something the player
    /// could have caught, so a successful trade is unmistakable.
    fn default() -> Self {
        TradeSpec {
            trainer: "ORACLE".into(),
            mons: vec![MonSpec {
                species: "Mew".into(),
                level: 5,
                nickname: "MEW".into(),
                ot_name: "GF".into(),
                ot_id: 22796,
                moves: vec!["Pound".into()],
                ..MonSpec::default()
            }],
            origin: "built-in default".into(),
        }
    }
}

/// Strip an optional surrounding pair of single or double quotes. Matches the
/// terminal config parser's behaviour: bare and quoted values are equivalent.
fn unquote(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner;
        }
    }
    v
}

impl TradeSpec {
    /// Parse a trade file. Unknown keys are an error rather than a silent
    /// no-op — a typo'd `speceis =` should say so, not hand over a default
    /// Pokémon and leave the user wondering.
    pub fn parse(text: &str, origin: &str) -> Result<TradeSpec, SourceError> {
        let mut spec = TradeSpec {
            trainer: "ORACLE".into(),
            mons: Vec::new(),
            origin: origin.to_string(),
        };

        for (n, raw) in text.lines().enumerate() {
            let line_no = n + 1;
            let line = match raw.find('#') {
                Some(i) => &raw[..i],
                None => raw,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = match line.split_once('=') {
                Some((k, v)) => (k.trim().to_ascii_lowercase(), unquote(v)),
                None => {
                    return Err(SourceError::Parse {
                        line: line_no,
                        message: format!("expected 'key = value', found '{line}'"),
                    })
                }
            };

            let err = |m: String| SourceError::Parse {
                line: line_no,
                message: m,
            };

            if key == "trainer" {
                spec.trainer = value.to_string();
                continue;
            }
            if key == "species" {
                if spec.mons.len() >= party::PARTY_LENGTH {
                    return Err(err(format!(
                        "more than {} Pokémon listed",
                        party::PARTY_LENGTH
                    )));
                }
                spec.mons.push(MonSpec {
                    species: value.to_string(),
                    ..MonSpec::default()
                });
                continue;
            }

            let mon = spec.mons.last_mut().ok_or_else(|| {
                err(format!("'{key}' appears before any 'species =' line"))
            })?;
            match key.as_str() {
                "level" => {
                    let l: u8 = value
                        .parse()
                        .map_err(|_| err(format!("level '{value}' is not a number")))?;
                    if !(1..=100).contains(&l) {
                        return Err(err(format!("level {l} is outside 1..=100")));
                    }
                    mon.level = l;
                }
                "nickname" => mon.nickname = value.to_string(),
                "ot_name" | "ot" => mon.ot_name = value.to_string(),
                "ot_id" | "trainer_id" => {
                    mon.ot_id = value
                        .parse()
                        .map_err(|_| err(format!("ot_id '{value}' is not a number")))?
                }
                "moves" | "move" => {
                    mon.moves = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if mon.moves.len() > 4 {
                        return Err(err("a Pokémon can know at most 4 moves".into()));
                    }
                }
                "dvs" | "ivs" => {
                    let parts: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
                    if parts.len() != 4 {
                        return Err(err(
                            "dvs needs four values: attack, defense, speed, special"
                                .into(),
                        ));
                    }
                    let mut v = [0u8; 4];
                    for (i, p) in parts.iter().enumerate() {
                        let d: u8 = p
                            .parse()
                            .map_err(|_| err(format!("DV '{p}' is not a number")))?;
                        if d > 15 {
                            return Err(err(format!("DV {d} is outside 0..=15")));
                        }
                        v[i] = d;
                    }
                    mon.dvs = (v[0], v[1], v[2], v[3]);
                }
                _ => {
                    return Err(err(format!("unknown key '{key}'")));
                }
            }
        }

        if spec.mons.is_empty() {
            return Err(SourceError::Parse {
                line: 0,
                message: "no 'species =' line — nothing to trade".into(),
            });
        }
        Ok(spec)
    }

    /// Read a trade file from disk.
    pub fn load(path: &std::path::Path) -> Result<TradeSpec, SourceError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            SourceError::Io(format!("cannot read {}: {e}", path.display()))
        })?;
        TradeSpec::parse(&text, &path.display().to_string())
    }
}

impl PokemonSource for TradeSpec {
    fn describe(&self) -> String {
        let names: Vec<&str> = self.mons.iter().map(|m| m.species.as_str()).collect();
        format!("{} ({})", names.join(", "), self.origin)
    }

    fn party(&self, tables: &RomTables) -> Result<Party, SourceError> {
        let mut mons = Vec::with_capacity(self.mons.len());
        for m in &self.mons {
            let info = tables.resolve_species(&m.species)?;
            let mut moves = [0u8; 4];
            if m.moves.is_empty() {
                // No moves given: a Pokémon with no moves is not well formed,
                // so fall back to the species' first level-up move, Pound.
                moves[0] = 1;
            } else {
                for (i, name) in m.moves.iter().take(4).enumerate() {
                    moves[i] = tables.resolve_move(name)?;
                }
            }
            mons.push(party::build_mon(
                tables,
                info.index,
                m.level,
                moves,
                m.dvs,
                m.ot_id,
                &m.ot_name,
                &m.nickname,
            )?);
        }
        let p = Party {
            trainer_name: self.trainer.clone(),
            mons,
        };
        // Build the blocks once here so a malformed party is reported at load
        // time, with the file name, rather than mid-trade.
        p.to_blocks()?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_file_parses() {
        let s = TradeSpec::parse("species = Mew\nlevel = 5\n", "test").unwrap();
        assert_eq!(s.mons.len(), 1);
        assert_eq!(s.mons[0].species, "Mew");
        assert_eq!(s.mons[0].level, 5);
    }

    #[test]
    fn comments_blank_lines_and_quotes_are_all_accepted() {
        let s = TradeSpec::parse(
            "# a comment\n\ntrainer = \"ORACLE\"\nspecies = 'Mew'  # inline\nmoves = Pound, Psychic\n",
            "test",
        )
        .unwrap();
        assert_eq!(s.trainer, "ORACLE");
        assert_eq!(s.mons[0].species, "Mew");
        assert_eq!(s.mons[0].moves, vec!["Pound", "Psychic"]);
    }

    #[test]
    fn several_pokemon_can_be_listed() {
        let s = TradeSpec::parse(
            "species = Mew\nlevel = 5\nspecies = Pikachu\nlevel = 25\n",
            "test",
        )
        .unwrap();
        assert_eq!(s.mons.len(), 2);
        assert_eq!(s.mons[1].species, "Pikachu");
        assert_eq!(s.mons[1].level, 25);
        // The level applies to the mon it follows, not the first one.
        assert_eq!(s.mons[0].level, 5);
    }

    #[test]
    fn a_typo_is_reported_rather_than_ignored() {
        let e = TradeSpec::parse("species = Mew\nspeceis = Mew\n", "test").unwrap_err();
        assert!(matches!(e, SourceError::Parse { line: 2, .. }), "{e}");
    }

    #[test]
    fn out_of_range_values_are_refused() {
        assert!(TradeSpec::parse("species = Mew\nlevel = 0\n", "t").is_err());
        assert!(TradeSpec::parse("species = Mew\nlevel = 200\n", "t").is_err());
        assert!(TradeSpec::parse("species = Mew\ndvs = 16,0,0,0\n", "t").is_err());
        assert!(TradeSpec::parse("species = Mew\ndvs = 1,2,3\n", "t").is_err());
    }

    #[test]
    fn a_key_before_any_species_is_an_error() {
        let e = TradeSpec::parse("level = 5\n", "test").unwrap_err();
        assert!(matches!(e, SourceError::Parse { line: 1, .. }));
    }

    #[test]
    fn an_empty_file_says_there_is_nothing_to_trade() {
        assert!(TradeSpec::parse("# nothing here\n", "test").is_err());
    }

    #[test]
    fn the_default_offer_is_the_event_mew() {
        let d = TradeSpec::default();
        assert_eq!(d.mons.len(), 1);
        assert_eq!(d.mons[0].species, "Mew");
        assert_eq!(d.mons[0].level, 5);
    }
}

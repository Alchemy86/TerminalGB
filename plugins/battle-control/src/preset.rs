//! Named battles, in a file the user hand-edits.
//!
//! The format is the same hand-rolled `key = value` shape the terminal
//! frontend's `config.toml` and the link partner's trade file use — no TOML
//! dependency, `#` starts a comment, values may be bare or quoted — plus
//! `[section]` headers, because the whole point is a *list* of named battles
//! one of which is triggered by name.
//!
//! ```text
//! [charizard-flamethrower]
//! description = Charizard flamethrowers a Pidgey
//!
//! player   = Charizard          # a `player` line starts a party member
//! level    = 50
//! moves    = Flamethrower
//! nickname = CHARIZARD
//!
//! wild     = Pidgey             # ...and `wild` / `trainer` the opposition
//! level    = 3
//!
//! seed     = 0x1234             # pin the RNG (see the module docs)
//!
//! do = await-battle             # the cinematic
//! do = await FIGHT
//! do = press a
//! do = await-overworld
//! ```
//!
//! A `player` / `wild` / `enemy` line starts a new Pokémon, and every key after
//! it describes *that* Pokémon, exactly as `species =` works in a trade file.
//! An unknown key is an error rather than a silent no-op: a typo'd `spceies =`
//! should say so, not quietly fight the wrong thing.

use super::script::Script;
use terminalgb_plugin_gen1_link::party::PARTY_LENGTH;

/// One requested Pokémon, before it is resolved against a cartridge.
///
/// Every optional field is an `Option` rather than a defaulted value on
/// purpose. For the *opposition* in a wild battle the engine generates the
/// Pokémon itself — random DVs, the level-up moveset for the level — and that
/// generation is more legitimate than anything this plugin could substitute.
/// So "the user did not say" and "the user said 15/15/15/15" have to be
/// different things: the first leaves the engine alone, the second overwrites
/// what it produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonPlan {
    /// A name (`Charizard`), a `#`-prefixed Pokédex number (`#6`), or a bare
    /// internal index — resolved by [`terminalgb_plugin_gen1_link::romdata`].
    pub species: String,
    pub level: u8,
    pub nickname: Option<String>,
    pub ot_name: Option<String>,
    pub ot_id: Option<u16>,
    pub moves: Option<Vec<String>>,
    /// Attack, Defense, Speed, Special. The HP DV is derived from these.
    pub dvs: Option<(u8, u8, u8, u8)>,
}

/// Level a Pokémon gets when a preset does not say. High enough that a battle
/// triggered on a whim is worth watching.
pub const DEFAULT_LEVEL: u8 = 50;

impl MonPlan {
    fn named(species: &str) -> MonPlan {
        MonPlan {
            species: species.to_string(),
            level: DEFAULT_LEVEL,
            ..MonPlan::default()
        }
    }

    /// Did the preset ask for anything the battle engine would not produce on
    /// its own? If not, a wild opponent needs no patching at all.
    pub fn overrides_engine(&self) -> bool {
        self.moves.is_some() || self.dvs.is_some() || self.nickname.is_some()
    }
}

/// Who is on the other side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opposition {
    /// A wild Pokémon: `wCurOpponent` is its species index and
    /// `wCurEnemyLevel` its level, which is all the engine needs.
    Wild(MonPlan),
    /// A trainer: `wCurOpponent` is the class plus `OPP_ID_OFFSET` and
    /// `wTrainerNo` picks which of that class's rosters to use.
    Trainer {
        /// Class name (resolved against the cartridge's own `TrainerNames`) or
        /// a bare class number.
        class: String,
        /// `wTrainerNo`, 1-based. The game's own numbering.
        roster: u8,
        /// If non-empty, the ROM roster is replaced with this team once
        /// `ReadTrainer` has run. Empty means "fight the real roster".
        team: Vec<MonPlan>,
    },
}

/// One named battle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub description: String,
    /// The party to give the player. Empty means "leave the party alone",
    /// which is the only form that is safe to fire at an arbitrary save.
    pub player: Vec<MonPlan>,
    pub opposition: Opposition,
    /// `wBattleType`: 0 normal, 1 the old man's tutorial, 2 Safari Zone.
    pub battle_type: u8,
    /// Written to `hRandomAdd`/`hRandomSub` immediately before the trigger.
    /// See [`super`] for exactly what this does and does not guarantee.
    pub seed: Option<u16>,
    pub script: Script,
    /// Where this came from, for the status line.
    pub origin: String,
}

impl Preset {
    /// One line for a menu or a log.
    pub fn summary(&self) -> String {
        let who = match &self.opposition {
            Opposition::Wild(m) => format!("wild {} Lv{}", m.species, m.level),
            Opposition::Trainer {
                class,
                roster,
                team,
            } => {
                if team.is_empty() {
                    format!("{class} #{roster}")
                } else {
                    format!("{class} #{roster} ({} custom)", team.len())
                }
            }
        };
        if self.description.is_empty() {
            who
        } else {
            format!("{} — {who}", self.description)
        }
    }

    /// Does firing this preset overwrite the player's party?
    pub fn touches_party(&self) -> bool {
        !self.player.is_empty()
    }
}

#[derive(Debug)]
pub enum PresetError {
    Parse {
        line: usize,
        message: String,
    },
    Io(String),
    /// Asked for a preset that is not in the file.
    Unknown {
        name: String,
        known: Vec<String>,
    },
}

impl std::fmt::Display for PresetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetError::Parse { line, message } => {
                write!(f, "battle preset line {line}: {message}")
            }
            PresetError::Io(e) => write!(f, "{e}"),
            PresetError::Unknown { name, known } => write!(
                f,
                "no battle preset called '{name}'; this file has: {}",
                known.join(", ")
            ),
        }
    }
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner;
        }
    }
    v
}

/// Which Pokémon the keys after a `level =` line belong to.
enum Cursor {
    None,
    Player(usize),
    Wild,
    Enemy(usize),
}

/// A whole preset file: an ordered list of named battles.
#[derive(Clone, Debug, Default)]
pub struct PresetFile {
    pub presets: Vec<Preset>,
}

impl PresetFile {
    /// The presets that ship with the plugin, so `one keypress` works before
    /// anybody has written a file.
    ///
    /// The first is the maintainer's own example, and it is first on purpose: it
    /// is the one that has to work.
    pub fn builtin() -> PresetFile {
        let text = include_str!("builtin.preset");
        PresetFile::parse(text, "built-in").expect("the built-in presets must parse")
    }

    pub fn load(path: &std::path::Path) -> Result<PresetFile, PresetError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            PresetError::Io(format!("cannot read {}: {e}", path.display()))
        })?;
        PresetFile::parse(&text, &path.display().to_string())
    }

    pub fn names(&self) -> Vec<String> {
        self.presets.iter().map(|p| p.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Result<&Preset, PresetError> {
        self.presets
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| PresetError::Unknown {
                name: name.to_string(),
                known: self.names(),
            })
    }

    pub fn parse(text: &str, origin: &str) -> Result<PresetFile, PresetError> {
        let mut out = PresetFile::default();
        // The in-progress preset, plus which mon subsequent keys apply to.
        let mut cur: Option<Preset> = None;
        let mut cursor = Cursor::None;
        let mut opposition: Option<Opposition> = None;

        macro_rules! finish {
            ($line:expr) => {
                if let Some(mut p) = cur.take() {
                    p.opposition = opposition.take().ok_or(PresetError::Parse {
                        line: $line,
                        message: format!(
                            "preset '{}' has no 'wild =' or 'trainer =' line, so \
                             there is nobody to fight",
                            p.name
                        ),
                    })?;
                    out.presets.push(p);
                }
            };
        }

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
            let err = |m: String| PresetError::Parse {
                line: line_no,
                message: m,
            };

            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                finish!(line_no);
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Err(err("a preset needs a name in its [brackets]".into()));
                }
                if out
                    .presets
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(&name))
                {
                    return Err(err(format!("two presets are called '{name}'")));
                }
                cur = Some(Preset {
                    name,
                    description: String::new(),
                    player: Vec::new(),
                    // Replaced by `finish!`; a preset with no opposition never
                    // reaches the output.
                    opposition: Opposition::Wild(MonPlan::default()),
                    battle_type: 0,
                    seed: None,
                    script: Script::default(),
                    origin: origin.to_string(),
                });
                cursor = Cursor::None;
                opposition = None;
                continue;
            }

            let (key, value) = match line.split_once('=') {
                Some((k, v)) => (k.trim().to_ascii_lowercase(), unquote(v)),
                None => {
                    return Err(err(format!("expected 'key = value', found '{line}'")))
                }
            };
            let p = cur.as_mut().ok_or_else(|| {
                err(format!("'{key}' appears before any [preset] header"))
            })?;

            match key.as_str() {
                "description" | "desc" => {
                    p.description = value.to_string();
                    continue;
                }
                "seed" => {
                    p.seed = Some(parse_u16(value).map_err(err)?);
                    continue;
                }
                "battle_type" => {
                    p.battle_type = match value.to_ascii_lowercase().as_str() {
                        "normal" | "0" => 0,
                        "old_man" | "oldman" | "1" => 1,
                        "safari" | "2" => 2,
                        other => {
                            return Err(err(format!(
                                "battle_type '{other}' is not normal, old_man or safari"
                            )))
                        }
                    };
                    continue;
                }
                "do" => {
                    p.script.push_line(value).map_err(err)?;
                    continue;
                }
                "player" => {
                    if p.player.len() >= PARTY_LENGTH {
                        return Err(err(format!(
                            "more than {PARTY_LENGTH} Pokémon on the player's side"
                        )));
                    }
                    p.player.push(MonPlan::named(value));
                    cursor = Cursor::Player(p.player.len() - 1);
                    continue;
                }
                "wild" => {
                    if opposition.is_some() {
                        return Err(err(
                            "this preset already has an opposition; a battle has \
                             one 'wild =' or one 'trainer =' line"
                                .into(),
                        ));
                    }
                    opposition = Some(Opposition::Wild(MonPlan::named(value)));
                    cursor = Cursor::Wild;
                    continue;
                }
                "trainer" => {
                    if opposition.is_some() {
                        return Err(err(
                            "this preset already has an opposition; a battle has \
                             one 'wild =' or one 'trainer =' line"
                                .into(),
                        ));
                    }
                    opposition = Some(Opposition::Trainer {
                        class: value.to_string(),
                        roster: 1,
                        team: Vec::new(),
                    });
                    cursor = Cursor::None;
                    continue;
                }
                "roster" => {
                    let r: u8 = value
                        .parse()
                        .map_err(|_| err(format!("roster '{value}' is not a number")))?;
                    match opposition.as_mut() {
                        Some(Opposition::Trainer { roster, .. }) => *roster = r,
                        _ => {
                            return Err(err(
                                "'roster' only means something after a 'trainer =' line"
                                    .into(),
                            ))
                        }
                    }
                    continue;
                }
                "enemy" => {
                    match opposition.as_mut() {
                        Some(Opposition::Trainer { team, .. }) => {
                            if team.len() >= PARTY_LENGTH {
                                return Err(err(format!(
                                    "more than {PARTY_LENGTH} Pokémon on the \
                                     trainer's team"
                                )));
                            }
                            team.push(MonPlan::named(value));
                            cursor = Cursor::Enemy(team.len() - 1);
                        }
                        _ => {
                            return Err(err(
                                "'enemy' lists a trainer's team, so it needs a \
                                 'trainer =' line first (for a wild battle, use \
                                 'wild =')"
                                    .into(),
                            ))
                        }
                    }
                    continue;
                }
                _ => {}
            }

            // Everything else describes the Pokémon the cursor is on.
            let mon: &mut MonPlan = match (&cursor, opposition.as_mut()) {
                (Cursor::Player(i), _) => &mut p.player[*i],
                (Cursor::Wild, Some(Opposition::Wild(m))) => m,
                (Cursor::Enemy(i), Some(Opposition::Trainer { team, .. })) => {
                    &mut team[*i]
                }
                _ => {
                    return Err(err(format!(
                        "'{key}' appears before any 'player =', 'wild =' or \
                         'enemy =' line, so there is no Pokémon for it to describe"
                    )))
                }
            };
            apply_mon_key(mon, &key, value).map_err(err)?;
        }
        finish!(text.lines().count());

        if out.presets.is_empty() {
            return Err(PresetError::Parse {
                line: 0,
                message: "no [preset] sections — nothing to trigger".into(),
            });
        }
        Ok(out)
    }
}

/// The per-Pokémon keys, shared by all three sides. Deliberately the same
/// spelling as the link partner's trade file.
fn apply_mon_key(mon: &mut MonPlan, key: &str, value: &str) -> Result<(), String> {
    match key {
        "level" | "lv" => {
            let l: u8 = value
                .parse()
                .map_err(|_| format!("level '{value}' is not a number"))?;
            if !(1..=100).contains(&l) {
                return Err(format!("level {l} is outside 1..=100"));
            }
            mon.level = l;
        }
        "nickname" | "nick" => mon.nickname = Some(value.to_string()),
        "ot_name" | "ot" => mon.ot_name = Some(value.to_string()),
        "ot_id" | "trainer_id" => {
            mon.ot_id = Some(
                value
                    .parse()
                    .map_err(|_| format!("ot_id '{value}' is not a number"))?,
            )
        }
        "moves" | "move" => {
            let list: Vec<String> = value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if list.len() > 4 {
                return Err("a Pokémon can know at most 4 moves".into());
            }
            mon.moves = Some(list);
        }
        "dvs" | "ivs" => {
            let parts: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
            if parts.len() != 4 {
                return Err(
                    "dvs needs four values: attack, defense, speed, special".into()
                );
            }
            let mut v = [0u8; 4];
            for (i, s) in parts.iter().enumerate() {
                let d: u8 = s.parse().map_err(|_| format!("DV '{s}' is not a number"))?;
                if d > 15 {
                    return Err(format!("DV {d} is outside 0..=15"));
                }
                v[i] = d;
            }
            mon.dvs = Some((v[0], v[1], v[2], v[3]));
        }
        other => return Err(format!("unknown key '{other}'")),
    }
    Ok(())
}

fn parse_u16(v: &str) -> Result<u16, String> {
    let t = v.trim();
    let parsed = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u16::from_str_radix(h, 16)
    } else if let Some(h) = t.strip_prefix('$') {
        u16::from_str_radix(h, 16)
    } else {
        t.parse::<u16>()
    };
    parsed.map_err(|_| format!("'{v}' is not a 16-bit number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_presets_parse_and_lead_with_the_maintainers_example() {
        let f = PresetFile::builtin();
        assert!(f.presets.len() >= 3, "{:?}", f.names());
        let first = &f.presets[0];
        assert_eq!(first.name, "charizard-flamethrower");
        assert_eq!(first.player.len(), 1);
        assert_eq!(first.player[0].species.to_ascii_lowercase(), "charizard");
        assert_eq!(first.player[0].moves.as_deref().unwrap()[0], "Flamethrower");
        match &first.opposition {
            Opposition::Wild(m) => {
                assert_eq!(m.species.to_ascii_lowercase(), "pidgey");
                assert_eq!(m.level, 3);
            }
            other => panic!("expected a wild Pidgey, got {other:?}"),
        }
        assert!(!first.script.is_empty(), "the example must be a cinematic");
        assert!(first.seed.is_some(), "a cinematic pins its seed");
        // And there is at least one trainer battle, with and without a custom
        // roster, because those are the other two shapes.
        assert!(f.presets.iter().any(|p| matches!(
            &p.opposition,
            Opposition::Trainer { team, .. } if team.is_empty()
        )));
        assert!(f.presets.iter().any(|p| matches!(
            &p.opposition,
            Opposition::Trainer { team, .. } if !team.is_empty()
        )));
    }

    #[test]
    fn keys_attach_to_the_pokemon_they_follow() {
        let f = PresetFile::parse(
            "[t]\nplayer = Charizard\nlevel = 50\nplayer = Pikachu\nlevel = 5\n\
             wild = Pidgey\nlevel = 3\n",
            "t",
        )
        .unwrap();
        let p = &f.presets[0];
        assert_eq!(p.player[0].level, 50);
        assert_eq!(p.player[1].level, 5);
        match &p.opposition {
            Opposition::Wild(m) => assert_eq!(m.level, 3),
            _ => panic!(),
        }
    }

    #[test]
    fn a_trainer_team_replaces_the_rom_roster_only_when_given() {
        let plain = PresetFile::parse("[a]\ntrainer = BROCK\nroster = 1\n", "t").unwrap();
        match &plain.presets[0].opposition {
            Opposition::Trainer {
                class,
                roster,
                team,
            } => {
                assert_eq!(class, "BROCK");
                assert_eq!(*roster, 1);
                assert!(team.is_empty());
            }
            _ => panic!(),
        }
        let custom = PresetFile::parse(
            "[a]\ntrainer = BROCK\nenemy = Onix\nlevel = 60\nenemy = Geodude\n",
            "t",
        )
        .unwrap();
        match &custom.presets[0].opposition {
            Opposition::Trainer { team, .. } => {
                assert_eq!(team.len(), 2);
                assert_eq!(team[0].level, 60);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn a_preset_with_nobody_to_fight_is_refused() {
        let e = PresetFile::parse("[a]\nplayer = Charizard\n", "t").unwrap_err();
        assert!(format!("{e}").contains("nobody to fight"), "{e}");
    }

    #[test]
    fn typos_are_reported_with_their_line() {
        let e = PresetFile::parse("[a]\nwild = Pidgey\nlevle = 3\n", "t").unwrap_err();
        assert!(matches!(e, PresetError::Parse { line: 3, .. }), "{e}");
        assert!(format!("{e}").contains("levle"), "{e}");
    }

    #[test]
    fn a_key_before_any_pokemon_says_so() {
        let e = PresetFile::parse("[a]\nlevel = 5\nwild = Pidgey\n", "t").unwrap_err();
        assert!(
            format!("{e}").contains("no Pokémon for it to describe"),
            "{e}"
        );
    }

    #[test]
    fn two_oppositions_are_refused() {
        let e =
            PresetFile::parse("[a]\nwild = Pidgey\ntrainer = BROCK\n", "t").unwrap_err();
        assert!(format!("{e}").contains("already has an opposition"), "{e}");
    }

    #[test]
    fn seeds_may_be_decimal_or_hex() {
        for (text, want) in [
            ("seed = 4660", 4660u16),
            ("seed = 0x1234", 0x1234),
            ("seed = $ABCD", 0xABCD),
        ] {
            let f =
                PresetFile::parse(&format!("[a]\nwild = Pidgey\n{text}\n"), "t").unwrap();
            assert_eq!(f.presets[0].seed, Some(want), "{text}");
        }
        assert!(PresetFile::parse("[a]\nwild = Pidgey\nseed = nope\n", "t").is_err());
    }

    #[test]
    fn an_unknown_preset_lists_the_ones_that_exist() {
        let f =
            PresetFile::parse("[a]\nwild = Pidgey\n[b]\nwild = Rattata\n", "t").unwrap();
        let e = f.get("c").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("'c'") && msg.contains("a, b"), "{msg}");
        assert!(f.get("A").is_ok(), "names are matched case-insensitively");
    }

    #[test]
    fn a_preset_without_a_player_line_leaves_the_party_alone() {
        let f = PresetFile::parse("[a]\nwild = Pidgey\nlevel = 3\n", "t").unwrap();
        assert!(!f.presets[0].touches_party());
        let g = PresetFile::parse("[a]\nplayer = Pikachu\nwild = Pidgey\n", "t").unwrap();
        assert!(g.presets[0].touches_party());
    }

    #[test]
    fn duplicate_names_are_refused() {
        let e = PresetFile::parse("[a]\nwild = X\n[a]\nwild = Y\n", "t").unwrap_err();
        assert!(format!("{e}").contains("two presets"), "{e}");
    }
}

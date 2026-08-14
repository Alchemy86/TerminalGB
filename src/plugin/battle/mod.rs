//! **Battle control: start any Generation 1 battle on demand, and script it.**
//!
//! Pokémon Red and Blue decide to start a battle by reading one byte. The
//! overworld loop tests `wCurOpponent` on every iteration and, if it is not
//! zero, calls `NewBattle` → `InitBattle`
//! ([`home/overworld.asm`](https://github.com/pret/pokered/blob/master/home/overworld.asm),
//! `.checkForOpponent`). Below `OPP_ID_OFFSET` (200) the byte is a species and
//! the engine builds a wild encounter; at or above it, it is a trainer class
//! plus that offset and the engine reads that class's roster out of the
//! cartridge. This is not a trick discovered by poking around — it is exactly
//! what the game's own debug menu does
//! ([`engine/debug/debug_menu.asm`](https://github.com/pret/pokered/blob/master/engine/debug/debug_menu.asm),
//! which gives the player a Rhydon and then writes `RHYDON` to `wCurOpponent`).
//!
//! So this plugin writes four bytes and lets the engine do everything else: the
//! transition, the sprite, the music, the AI, the animations, the sound and the
//! end-of-battle bookkeeping are all the real ones. The full technique, and
//! what else the same approach reaches, is written up in
//! [`docs/gen1/battle-control.md`](../../docs/gen1/battle-control.md).
//!
//! # What it guarantees about your save
//!
//! Three things, in the order they matter:
//!
//! 1. **It never writes cartridge RAM.** A battery-backed save file is written
//!    from cartridge RAM (`$A000-$BFFF`) by [`crate::mbc::battery`]. Every
//!    write this plugin makes goes through [`BattlePlugin::write`], which
//!    refuses any address outside WRAM and HRAM
//!    ([`wram::is_writable`]) and counts the refusal. A party injected into
//!    WRAM can only reach a `.sav` if the *player* then walks into a Pokémon
//!    Center and saves — which is a decision, not an accident.
//! 2. **Everything it writes, it puts back.** Before overwriting the player's
//!    party it snapshots the exact byte ranges it is about to change, and
//!    restores them when the battle ends. A preset with no `player =` line
//!    never touches the party at all, so there is nothing to restore.
//! 3. **Nothing fires by itself.** The plugin is not attached unless it is
//!    named in `plugins = …` / `GB_PLUGINS`, it does nothing until
//!    [`BattlePlugin::arm`] is called, and while it is attached the frontend
//!    shows it in the status panel with the `!` write marker.
//!
//! # Which games
//!
//! English Pokémon **Red and Blue** only. The cartridge is identified by title
//! *and* header global checksum ([`crate::trainer::Game::detect`]) and then
//! every address the plugin writes is re-derived from the cartridge's own
//! machine code ([`wram::verify`]). Yellow moved its WRAM and is refused;
//! Generation 2 changed the structures and is refused. A refusal says which
//! check failed — writing to the wrong address is the failure mode this whole
//! module is arranged to avoid.

pub mod preset;
pub mod script;
pub mod wram;

use crate::input::KeypadKey;
use crate::plugin::gen1::party::{self, Mon};
use crate::plugin::gen1::romdata::{RomError, RomTables};
use crate::plugin::gen1::text;
use crate::plugin::{Capability, Event, Machine, Plugin};
use crate::trainer::Game;
use preset::{MonPlan, Opposition, Preset, PresetError, PresetFile};

/// The plugin's registered name.
pub const NAME: &str = "gen1-battle";

/// How long the plugin keeps re-applying an opposition override after the
/// battle flag appears.
///
/// The engine fills `wEnemyMon` (wild) or `wEnemyMons` (trainer) inside
/// `InitBattle`, which is not synchronised to a frame boundary — so a plugin
/// that patched once, on the first frame it saw the flag, could be overwritten
/// by the rest of `LoadEnemyMonData` running immediately afterwards. The fix is
/// not a bigger delay but a **read-back**: keep writing until the bytes are
/// still ours at the *start* of a frame, which proves the engine has stopped
/// touching them. This is only the safety net for a game that never settles.
const PATCH_WINDOW_FRAMES: u32 = 40;

/// Why a battle could not be set up. Every variant is something a user can act
/// on, which is the bar for a plugin that would otherwise fail silently.
#[derive(Debug)]
pub enum BattleError {
    Rom(RomError),
    Preset(PresetError),
    Party(party::PartyError),
    Verify(wram::VerifyError),
    /// The cartridge is not one this plugin knows.
    UnsupportedGame(String),
    /// A trainer class name that is not in this cartridge's `TrainerNames`.
    UnknownTrainerClass {
        name: String,
        known: usize,
    },
    /// A trainer class number outside the range the cartridge has data for.
    TrainerClassOutOfRange(u16),
}

impl std::fmt::Display for BattleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BattleError::Rom(e) => write!(f, "{e}"),
            BattleError::Preset(e) => write!(f, "{e}"),
            BattleError::Party(e) => write!(f, "{e}"),
            BattleError::Verify(e) => write!(f, "{e}"),
            BattleError::UnsupportedGame(t) => write!(
                f,
                "'{t}' is not English Pokémon Red or Blue — the battle plugin \
                 writes to fixed WRAM addresses and will not guess at another \
                 game's layout"
            ),
            BattleError::UnknownTrainerClass { name, known } => write!(
                f,
                "'{name}' is not one of this cartridge's {known} trainer classes"
            ),
            BattleError::TrainerClassOutOfRange(n) => write!(
                f,
                "trainer class {n} is outside 1..=55; above that the value \
                 would not fit in wCurOpponent"
            ),
        }
    }
}

impl From<RomError> for BattleError {
    fn from(e: RomError) -> Self {
        BattleError::Rom(e)
    }
}
impl From<PresetError> for BattleError {
    fn from(e: PresetError) -> Self {
        BattleError::Preset(e)
    }
}
impl From<party::PartyError> for BattleError {
    fn from(e: party::PartyError) -> Self {
        BattleError::Party(e)
    }
}
impl From<wram::VerifyError> for BattleError {
    fn from(e: wram::VerifyError) -> Self {
        BattleError::Verify(e)
    }
}

/// A run of bytes destined for one address, and (once written) what was there
/// before. Snapshot and restore are the same operation on the same list, which
/// is what makes "everything it writes, it puts back" checkable rather than
/// hopeful.
#[derive(Clone, Debug)]
struct Block {
    addr: u16,
    bytes: Vec<u8>,
}

/// Everything the plugin will write, resolved against the cartridge, before a
/// single byte goes anywhere. Building this can fail; writing it cannot.
#[derive(Clone, Debug)]
struct Plan {
    /// `wCurOpponent`: a species index, or a trainer class + `OPP_ID_OFFSET`.
    trigger: u8,
    /// `wCurEnemyLevel`.
    level: u8,
    /// `wBattleType`.
    battle_type: u8,
    /// `wTrainerNo`, only meaningful for a trainer battle.
    trainer_no: u8,
    seed: Option<u16>,
    /// The player's party, or empty to leave it alone. Kept as [`Mon`]s rather
    /// than bytes because two fields cannot be filled in until the game is
    /// running — see [`BattlePlugin::player_blocks`].
    player: Vec<Mon>,
    /// Per slot: did the preset state an original trainer of its own?
    player_ot_explicit: Vec<bool>,
    /// The 29-byte `battle_struct` + nickname for a wild opponent whose DVs,
    /// moves or nickname the preset overrode.
    wild_override: Vec<Block>,
    /// A whole replacement enemy party for a trainer battle.
    enemy_party: Vec<Block>,
    /// One line describing the fight, for the log.
    summary: String,
}

/// Where a triggered battle has got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing armed.
    Idle,
    /// Armed, waiting for the game to be somewhere it is safe to fire.
    Waiting,
    /// `wCurOpponent` written; waiting for the engine to notice.
    Triggered,
    /// In the battle. Applying any opposition override, then running the
    /// script if there is one.
    Fighting,
    /// The battle ended and anything we overwrote has been put back.
    Done,
    /// Gave up. The string is shown to the user.
    Failed(String),
}

impl Phase {
    pub fn label(&self) -> String {
        match self {
            Phase::Idle => "idle".into(),
            Phase::Waiting => "waiting for the overworld".into(),
            Phase::Triggered => "starting".into(),
            Phase::Fighting => "in battle".into(),
            Phase::Done => "done".into(),
            Phase::Failed(m) => format!("failed: {m}"),
        }
    }
}

/// The live half of a triggered battle.
struct Active {
    name: String,
    plan: Plan,
    phase: Phase,
    /// Byte ranges we changed and their original contents.
    restore: Vec<Block>,
    /// Frames spent in the current phase.
    frames: u32,
    runner: Option<script::Runner>,
    /// The button the script asked us to hold last frame, so it can be let go.
    held: Option<KeypadKey>,
    /// Whether the opposition override has read back unchanged.
    patch_settled: bool,
    patch_frames: u32,
}

/// The plugin.
pub struct BattlePlugin {
    tables: RomTables,
    /// The cartridge's own `TrainerNames`, so a preset can say `BROCK`.
    classes: Vec<String>,
    presets: PresetFile,
    /// `None` until [`Plugin::on_attach`] has looked at the cartridge.
    game: Option<Game>,
    /// Why the plugin refuses to act, if it does.
    refusal: Option<String>,
    active: Option<Active>,
    /// Which preset the "one keypress" trigger will fire.
    selected: usize,
    /// Sticky record of what happened, for a headless run to assert on and for
    /// the TUI to toast.
    log: Vec<String>,
    /// Writes the plugin itself refused because the address was outside WRAM
    /// and HRAM. Must always be zero; surfaced so it cannot be zero by luck.
    refused_writes: u32,
    battles_started: u32,
}

impl BattlePlugin {
    /// Build from a ROM image and a preset file (or the built-in list).
    ///
    /// The ROM image is needed rather than borrowed memory because the species,
    /// base-stat, move and trainer-name tables live in banks the running game
    /// maps only while it is using them — the same reason
    /// [`crate::plugin::gen1::romdata`] takes one.
    pub fn new(rom: Vec<u8>, presets: PresetFile) -> Result<BattlePlugin, BattleError> {
        // Re-derive every address from this cartridge's machine code before
        // anything else. If this fails the plugin does not exist, rather than
        // existing and refusing later.
        wram::verify(&rom)?;
        let classes = trainer_class_names(&rom);
        let tables = RomTables::locate(rom)?;
        Ok(BattlePlugin {
            tables,
            classes,
            presets,
            game: None,
            refusal: None,
            active: None,
            selected: 0,
            log: Vec::new(),
            refused_writes: 0,
            battles_started: 0,
        })
    }

    /// Convenience: a ROM plus an optional preset-file path, falling back to
    /// the built-in presets.
    pub fn from_preset_file(
        rom: Vec<u8>,
        path: Option<&std::path::Path>,
    ) -> Result<BattlePlugin, BattleError> {
        let presets = match path {
            Some(p) => PresetFile::load(p)?,
            None => PresetFile::builtin(),
        };
        BattlePlugin::new(rom, presets)
    }

    pub fn preset_names(&self) -> Vec<String> {
        self.presets.names()
    }

    /// The preset the one-touch trigger will fire.
    pub fn selected(&self) -> Option<&Preset> {
        self.presets.presets.get(self.selected)
    }

    /// Move the selection on by one, wrapping. This is the other half of "one
    /// touch": a key to choose and a key to fire.
    pub fn select_next(&mut self) -> Option<&Preset> {
        if self.presets.presets.is_empty() {
            return None;
        }
        self.selected = (self.selected + 1) % self.presets.presets.len();
        self.selected()
    }

    pub fn select(&mut self, name: &str) -> Result<&Preset, BattleError> {
        let i = self
            .presets
            .presets
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                BattleError::Preset(PresetError::Unknown {
                    name: name.to_string(),
                    known: self.presets.names(),
                })
            })?;
        self.selected = i;
        Ok(&self.presets.presets[i])
    }

    /// Arm the currently selected preset. Nothing is written yet — the plugin
    /// waits for a frame on which the game is somewhere a battle can start.
    pub fn arm_selected(&mut self) -> Result<(), BattleError> {
        let name = match self.selected() {
            Some(p) => p.name.clone(),
            None => {
                return Err(BattleError::Preset(PresetError::Unknown {
                    name: "<none>".into(),
                    known: Vec::new(),
                }))
            }
        };
        self.arm(&name)
    }

    /// Arm a preset by name.
    pub fn arm(&mut self, name: &str) -> Result<(), BattleError> {
        if let Some(r) = &self.refusal {
            return Err(BattleError::UnsupportedGame(r.clone()));
        }
        let preset = self.presets.get(name)?.clone();
        let plan = self.build_plan(&preset)?;
        self.log
            .push(format!("armed '{}': {}", preset.name, plan.summary));
        self.active = Some(Active {
            name: preset.name.clone(),
            plan,
            phase: Phase::Waiting,
            restore: Vec::new(),
            frames: 0,
            runner: if preset.script.is_empty() {
                None
            } else {
                Some(script::Runner::new(preset.script.clone()))
            },
            held: None,
            patch_settled: false,
            patch_frames: 0,
        });
        Ok(())
    }

    /// Abandon a pending or running battle and put back anything already
    /// written. Safe to call at any time.
    pub fn cancel(&mut self) {
        if self.active.is_some() {
            self.log.push("cancelled".into());
        }
        // The restore itself needs a `Machine`, so mark it and let the next
        // frame do the writing; if no frame ever comes, nothing was written
        // that a reset would not also discard.
        if let Some(a) = self.active.as_mut() {
            a.phase = Phase::Failed("cancelled".into());
        }
    }

    pub fn phase(&self) -> Phase {
        self.active
            .as_ref()
            .map(|a| a.phase.clone())
            .unwrap_or(Phase::Idle)
    }

    pub fn battles_started(&self) -> u32 {
        self.battles_started
    }

    /// The preset currently armed or running, if any. Not the same as
    /// [`BattlePlugin::selected`]: the selection can be cycled while a battle
    /// is under way.
    pub fn active_preset(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.name.as_str())
    }

    /// Writes refused because the address was outside WRAM/HRAM. Always zero;
    /// exposed so a test can prove it is zero for the right reason.
    pub fn refused_writes(&self) -> u32 {
        self.refused_writes
    }

    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// Progress through the cinematic, if one is running.
    pub fn script_progress(&self) -> Option<(usize, usize)> {
        self.active.as_ref()?.runner.as_ref().map(|r| r.progress())
    }

    /// `true` once the armed battle has finished — the signal a headless
    /// runner waits on.
    pub fn is_finished(&self) -> bool {
        matches!(self.phase(), Phase::Done | Phase::Failed(_))
    }

    // -- planning ---------------------------------------------------------

    fn build_plan(&self, p: &Preset) -> Result<Plan, BattleError> {
        let mut plan = Plan {
            trigger: 0,
            level: preset::DEFAULT_LEVEL,
            battle_type: p.battle_type,
            trainer_no: 1,
            seed: p.seed,
            player: Vec::new(),
            player_ot_explicit: Vec::new(),
            wild_override: Vec::new(),
            enemy_party: Vec::new(),
            summary: String::new(),
        };

        if !p.player.is_empty() {
            plan.player = self.resolve_team(&p.player, "")?;
            plan.player_ot_explicit = p
                .player
                .iter()
                .map(|m| m.ot_id.is_some() || m.ot_name.is_some())
                .collect();
        }

        match &p.opposition {
            Opposition::Wild(m) => {
                let info = self.tables.resolve_species(&m.species)?;
                plan.trigger = info.index;
                plan.level = m.level;
                plan.summary = format!("wild {} Lv{}", info.name, m.level);
                if m.overrides_engine() {
                    let mon = self.build_one(m, "", 0)?;
                    plan.wild_override.push(Block {
                        addr: wram::addr::ENEMY_MON,
                        bytes: battle_struct(&mon).to_vec(),
                    });
                    let nick = m.nickname.clone().unwrap_or_else(|| info.name.clone());
                    plan.wild_override.push(Block {
                        addr: wram::addr::ENEMY_MON_NICK,
                        bytes: text::encode_fixed(&nick, wram::NAME_LENGTH),
                    });
                }
            }
            Opposition::Trainer {
                class,
                roster,
                team,
            } => {
                let id = self.resolve_class(class)?;
                plan.trigger = wram::OPP_ID_OFFSET
                    .checked_add(id)
                    .ok_or(BattleError::TrainerClassOutOfRange(id as u16))?;
                plan.trainer_no = (*roster).max(1);
                let label = self
                    .classes
                    .get(id as usize - 1)
                    .cloned()
                    .unwrap_or_else(|| format!("class {id}"));
                plan.summary = format!("{label} #{}", plan.trainer_no);
                if !team.is_empty() {
                    let mons = self.resolve_team(team, &label)?;
                    plan.summary = format!(
                        "{label} #{} with {} custom",
                        plan.trainer_no,
                        mons.len()
                    );
                    plan.enemy_party = party_blocks(
                        &mons,
                        wram::addr::ENEMY_PARTY_COUNT,
                        wram::addr::ENEMY_PARTY_SPECIES,
                        wram::addr::ENEMY_MONS,
                        wram::addr::ENEMY_MON_OT,
                        wram::addr::ENEMY_MON_NICKS,
                    );
                }
            }
        }
        Ok(plan)
    }

    /// Turn the planned party into byte blocks, filling in the original
    /// trainer from the *running game* for every slot the preset did not state
    /// one for.
    ///
    /// This is not cosmetic. `CheckForDisobedience`
    /// (`engine/battle/core.asm`) compares `wPartyMon1OTID` with `wPlayerID`
    /// and, if they differ, treats the Pokémon as **traded** — at which point a
    /// player with no badges is only obeyed up to level 10. Inject a level 50
    /// Charizard with somebody else's trainer id and the game answers
    /// "CHARIZARD won't obey!" and the turn is wasted. Only the *id* is
    /// compared, but the name is copied too so the summary screen agrees with
    /// it.
    fn player_blocks(&self, plan: &Plan, m: &mut Machine) -> Vec<Block> {
        let id = m
            .peek_be16(crate::trainer::gen1::addr::PLAYER_ID)
            .unwrap_or(0);
        let name_bytes =
            m.peek_range(crate::trainer::gen1::addr::PLAYER_NAME, wram::NAME_LENGTH);
        let name = text::decode(&name_bytes);
        let mut mons = plan.player.clone();
        for (i, mon) in mons.iter_mut().enumerate() {
            if plan.player_ot_explicit.get(i).copied().unwrap_or(false) {
                continue;
            }
            mon.ot_id = id;
            mon.ot_name = name.clone();
        }
        party_blocks(
            &mons,
            wram::addr::PARTY_COUNT,
            wram::addr::PARTY_SPECIES,
            wram::addr::PARTY_MONS,
            wram::addr::PARTY_MON_OT,
            wram::addr::PARTY_MON_NICKS,
        )
    }

    fn resolve_team(&self, plans: &[MonPlan], ot: &str) -> Result<Vec<Mon>, BattleError> {
        plans.iter().map(|m| self.build_one(m, ot, 0)).collect()
    }

    /// Resolve one requested Pokémon into a fully formed [`Mon`].
    ///
    /// "Fully formed" is the hard part and it is all delegated to
    /// [`party::build_mon`], the same function the link partner uses to put a
    /// Pokémon on the wire: types, catch rate and base stats come from the
    /// cartridge, the five stats are computed with the Gen 1 formula for the
    /// level and DVs, the experience is exactly the total for that level on
    /// that species' growth curve, and PP comes from the move table. A battle
    /// injected with anything less coherent glitches in ways that look like the
    /// emulator being wrong.
    fn build_one(
        &self,
        m: &MonPlan,
        ot_name: &str,
        ot_id: u16,
    ) -> Result<Mon, BattleError> {
        let info = self.tables.resolve_species(&m.species)?;
        let mut moves = [0u8; 4];
        match &m.moves {
            Some(list) if !list.is_empty() => {
                for (i, name) in list.iter().take(4).enumerate() {
                    moves[i] = self.tables.resolve_move(name)?;
                }
            }
            // Nothing stated: use the species' own level 1 learnset from the
            // cartridge rather than inventing something.
            _ => moves = self.tables.base_moves(info.index),
        }
        let dvs = m.dvs.unwrap_or((15, 15, 15, 15));
        let mon = party::build_mon(
            &self.tables,
            info.index,
            m.level,
            moves,
            dvs,
            m.ot_id.unwrap_or(ot_id),
            m.ot_name.as_deref().unwrap_or(ot_name),
            m.nickname.as_deref().unwrap_or(""),
        )?;
        Ok(mon)
    }

    /// A trainer class by name (from the cartridge's own table) or by number.
    fn resolve_class(&self, s: &str) -> Result<u8, BattleError> {
        let t = s.trim();
        if let Ok(n) = t.parse::<u16>() {
            if n == 0 || n > 55 {
                return Err(BattleError::TrainerClassOutOfRange(n));
            }
            return Ok(n as u8);
        }
        let want = normalise_class(t);
        for (i, name) in self.classes.iter().enumerate() {
            if normalise_class(name) == want {
                return Ok(i as u8 + 1);
            }
        }
        Err(BattleError::UnknownTrainerClass {
            name: s.to_string(),
            known: self.classes.len(),
        })
    }

    // -- writing ----------------------------------------------------------

    /// The single door every write goes through.
    ///
    /// It is not a formality: it is the mechanism behind guarantee 1 in the
    /// module docs. Nothing in this plugin calls [`Machine::poke`] directly.
    fn write(&mut self, m: &mut Machine, addr: u16, value: u8) {
        if !wram::is_writable(addr) {
            self.refused_writes += 1;
            return;
        }
        m.poke(addr, value);
    }

    /// Read the bytes a block covers, so they can be put back.
    fn snapshot(m: &mut Machine, block: &Block) -> Block {
        Block {
            addr: block.addr,
            bytes: m.peek_range(block.addr, block.bytes.len()),
        }
    }

    fn write_blocks(&mut self, m: &mut Machine, blocks: &[Block]) {
        for b in blocks {
            for (i, &v) in b.bytes.iter().enumerate() {
                self.write(m, b.addr.wrapping_add(i as u16), v);
            }
        }
    }

    /// Have the bytes we wrote survived a whole frame untouched?
    fn blocks_intact(m: &mut Machine, blocks: &[Block]) -> bool {
        blocks
            .iter()
            .all(|b| m.peek_range(b.addr, b.bytes.len()) == b.bytes)
    }

    // -- the per-frame state machine ---------------------------------------

    fn on_frame(&mut self, m: &mut Machine) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        active.frames += 1;
        match active.phase.clone() {
            Phase::Waiting => self.step_waiting(&mut active, m),
            Phase::Triggered => self.step_triggered(&mut active, m),
            Phase::Fighting => self.step_fighting(&mut active, m),
            Phase::Failed(_) => {
                // Cancelled, or gave up: put everything back, then sit in the
                // failed phase rather than vanishing. A plugin that silently
                // returns to "idle" after refusing is one whose refusal the
                // user never sees.
                let restore = std::mem::take(&mut active.restore);
                self.write_blocks(m, &restore);
                self.release(&mut active, m);
            }
            Phase::Idle | Phase::Done => {}
        }
        self.active = Some(active);
    }

    /// Is the game somewhere a battle can be started from?
    ///
    /// `wCurOpponent` is only read by the overworld loop, so writing it inside
    /// a menu or a cutscene is harmless — it simply fires later, which is
    /// surprising. Waiting for the game to be out of a battle and off the link
    /// makes "one keypress" mean "the next time you are walking around".
    fn step_waiting(&mut self, a: &mut Active, m: &mut Machine) {
        let in_battle = m.peek(wram::addr::IS_IN_BATTLE).unwrap_or(0);
        let link = m.peek(wram::addr::LINK_STATE).unwrap_or(0);
        let party = m.peek(wram::addr::PARTY_COUNT).unwrap_or(0);
        if in_battle != 0 || link != 0 {
            if a.frames > 900 {
                a.phase =
                    Phase::Failed("the game never returned to the overworld".into());
            }
            return;
        }
        // A battle with no Pokémon on the player's side is an instant black-out
        // and looks exactly like a crash. If the preset supplies a party this
        // is moot; if it does not, the save has to already have one.
        if a.plan.player.is_empty() && (party == 0 || party > 6) {
            if a.frames > 900 {
                a.phase = Phase::Failed(
                    "this save has no Pokémon, and the preset does not supply one".into(),
                );
            }
            return;
        }

        // Snapshot before the first byte moves.
        let player = if a.plan.player.is_empty() {
            Vec::new()
        } else {
            self.player_blocks(&a.plan, m)
        };
        for b in &player {
            let s = Self::snapshot(m, b);
            a.restore.push(s);
        }
        let plan = a.plan.clone();
        self.write_blocks(m, &player);
        if let Some(seed) = plan.seed {
            self.write(m, wram::addr::RANDOM_ADD, (seed >> 8) as u8);
            self.write(m, wram::addr::RANDOM_SUB, seed as u8);
        }
        self.write(m, wram::addr::CUR_ENEMY_LEVEL, plan.level);
        self.write(m, wram::addr::BATTLE_TYPE, plan.battle_type);
        self.write(m, wram::addr::TRAINER_NO, plan.trainer_no);
        // Last, because this is the byte the overworld loop is watching.
        self.write(m, wram::addr::CUR_OPPONENT, plan.trigger);
        a.phase = Phase::Triggered;
        a.frames = 0;
        self.battles_started += 1;
        self.log.push(format!("triggered {}", plan.summary));
    }

    fn step_triggered(&mut self, a: &mut Active, m: &mut Machine) {
        if m.peek(wram::addr::IS_IN_BATTLE).unwrap_or(0) != 0 {
            a.phase = Phase::Fighting;
            a.frames = 0;
            a.patch_frames = 0;
            a.patch_settled =
                a.plan.wild_override.is_empty() && a.plan.enemy_party.is_empty();
            self.log.push("battle started".into());
            return;
        }
        if a.frames > 600 {
            // `NewBattle` refuses on a dungeon warp, while the game is moving
            // the player itself, and where a map script has set BIT_NO_BATTLES
            // (`home/overworld.asm`). Say so rather than hanging.
            a.phase = Phase::Failed(
                "the overworld loop did not pick the battle up — the player may \
                 be mid-cutscene, on a warp tile, or on a map that suppresses \
                 battles"
                    .into(),
            );
        }
    }

    fn step_fighting(&mut self, a: &mut Active, m: &mut Machine) {
        // 1. Opposition override, until it reads back unchanged.
        if !a.patch_settled {
            a.patch_frames += 1;
            let wild = a.plan.wild_override.clone();
            let team = a.plan.enemy_party.clone();
            let intact = Self::blocks_intact(m, &wild) && Self::blocks_intact(m, &team);
            if intact && a.patch_frames > 1 {
                a.patch_settled = true;
                self.log.push("opposition set".into());
            } else if a.patch_frames > PATCH_WINDOW_FRAMES {
                a.patch_settled = true;
                self.log
                    .push("opposition override did not settle; left as generated".into());
            } else {
                self.write_blocks(m, &wild);
                self.write_blocks(m, &team);
            }
        }

        // 2. The cinematic.
        if let Some(mut runner) = a.runner.take() {
            // 360 peeks per frame is cheap but not free, and only two steps
            // look at the screen at all.
            let needs_screen = matches!(
                runner.current(),
                Some(script::Step::Await { .. } | script::Step::PressUntil { .. })
            );
            let screen = if needs_screen {
                wram::read_screen(m)
            } else {
                String::new()
            };
            let in_battle = m.peek(wram::addr::IS_IN_BATTLE).unwrap_or(0) != 0;
            let before = runner.progress().0;
            let pad = runner.tick(&script::Observation {
                screen: &screen,
                in_battle,
            });
            // Log each step as it completes. A cinematic that stalls is always
            // debugged by asking "which step, and what was on screen then?", so
            // the log answers both without a rerun.
            if runner.progress().0 != before {
                if let Some(step) = runner.script_step(before) {
                    self.log
                        .push(format!("  step {}: {}", before + 1, step.label()));
                }
            }
            self.apply_pad(a, m, pad.key);
            if let Some(msg) = runner.timed_out() {
                let msg = msg.to_string();
                a.phase = Phase::Failed(msg.clone());
                self.log.push(msg);
            }
            a.runner = Some(runner);
        }

        // 3. The end.
        if m.peek(wram::addr::IS_IN_BATTLE).unwrap_or(0) == 0 && a.frames > 2 {
            self.release(a, m);
            let restore = std::mem::take(&mut a.restore);
            let n = restore.len();
            self.write_blocks(m, &restore);
            a.phase = Phase::Done;
            self.log.push(if n > 0 {
                format!("battle over; {n} byte ranges restored")
            } else {
                "battle over; nothing to restore".into()
            });
        } else if a.frames > 60 * 60 * 10 {
            a.phase = Phase::Failed("the battle ran for ten minutes".into());
        }
    }

    fn apply_pad(&mut self, a: &mut Active, m: &mut Machine, key: Option<KeypadKey>) {
        if a.held == key {
            return;
        }
        if let Some(old) = a.held.take() {
            m.set_button(old, false);
        }
        if let Some(k) = key {
            m.set_button(k, true);
            a.held = Some(k);
        }
    }

    fn release(&mut self, a: &mut Active, m: &mut Machine) {
        if let Some(k) = a.held.take() {
            m.set_button(k, false);
        }
    }
}

/// Turn a resolved team into the byte blocks a Gen 1 party occupies.
///
/// The same shape serves the player's party and a trainer's, because they are
/// the same structures at different addresses — `wPartyMons` and `wEnemyMons`
/// are both six `party_struct`s (`macros/ram.asm`).
fn party_blocks(
    mons: &[Mon],
    count: u16,
    species: u16,
    structs: u16,
    ot: u16,
    nicks: u16,
) -> Vec<Block> {
    let n = mons.len().min(wram::PARTY_LENGTH);
    let mut species_list = vec![0xFFu8; wram::PARTY_LENGTH + 1];
    for (i, mon) in mons.iter().take(n).enumerate() {
        species_list[i] = mon.species;
    }
    let mut structs_bytes =
        Vec::with_capacity(wram::PARTY_LENGTH * wram::PARTY_STRUCT_LENGTH);
    let mut ot_bytes = Vec::with_capacity(wram::PARTY_LENGTH * wram::NAME_LENGTH);
    let mut nick_bytes = Vec::with_capacity(wram::PARTY_LENGTH * wram::NAME_LENGTH);
    for i in 0..wram::PARTY_LENGTH {
        match mons.get(i) {
            Some(mon) => {
                structs_bytes.extend_from_slice(&mon.to_bytes());
                ot_bytes.extend_from_slice(&text::encode_fixed(
                    &mon.ot_name,
                    wram::NAME_LENGTH,
                ));
                nick_bytes.extend_from_slice(&text::encode_fixed(
                    &mon.nickname,
                    wram::NAME_LENGTH,
                ));
            }
            None => {
                // Empty slots are zeroed rather than left as whatever the save
                // had: a party structure that is half old data and half new is
                // exactly the incoherent state that glitches.
                structs_bytes
                    .extend(std::iter::repeat(0).take(wram::PARTY_STRUCT_LENGTH));
                ot_bytes
                    .extend(std::iter::repeat(text::TERMINATOR).take(wram::NAME_LENGTH));
                nick_bytes
                    .extend(std::iter::repeat(text::TERMINATOR).take(wram::NAME_LENGTH));
            }
        }
    }
    vec![
        Block {
            addr: count,
            bytes: vec![n as u8],
        },
        Block {
            addr: species,
            bytes: species_list,
        },
        Block {
            addr: structs,
            bytes: structs_bytes,
        },
        Block {
            addr: ot,
            bytes: ot_bytes,
        },
        Block {
            addr: nicks,
            bytes: nick_bytes,
        },
    ]
}

/// A [`Mon`] in the 29-byte in-battle form (`battle_struct`, `macros/ram.asm`).
///
/// The party form and the battle form are *different*: the battle struct drops
/// original-trainer id and experience, so level and the five stats sit at
/// different offsets. Confusing the two is the classic way to produce a
/// level 6400 Pokémon.
fn battle_struct(m: &Mon) -> [u8; wram::BATTLE_STRUCT_LENGTH] {
    use wram::battle_field as f;
    let mut b = [0u8; wram::BATTLE_STRUCT_LENGTH];
    b[f::SPECIES] = m.species;
    b[f::HP..f::HP + 2].copy_from_slice(&m.current_hp.to_be_bytes());
    b[f::BOX_LEVEL] = m.level;
    b[f::STATUS] = m.status;
    b[f::TYPE1] = m.type1;
    b[f::TYPE2] = m.type2;
    b[f::CATCH_RATE] = m.catch_rate;
    b[f::MOVES..f::MOVES + 4].copy_from_slice(&m.moves);
    b[f::DVS] = (m.dv_attack << 4) | (m.dv_defense & 0x0F);
    b[f::DVS + 1] = (m.dv_speed << 4) | (m.dv_special & 0x0F);
    b[f::LEVEL] = m.level;
    b[f::MAX_HP..f::MAX_HP + 2].copy_from_slice(&m.stats.hp.to_be_bytes());
    b[f::ATTACK..f::ATTACK + 2].copy_from_slice(&m.stats.attack.to_be_bytes());
    b[f::DEFENSE..f::DEFENSE + 2].copy_from_slice(&m.stats.defense.to_be_bytes());
    b[f::SPEED..f::SPEED + 2].copy_from_slice(&m.stats.speed.to_be_bytes());
    b[f::SPECIAL..f::SPECIAL + 2].copy_from_slice(&m.stats.special.to_be_bytes());
    b[f::PP..f::PP + 4].copy_from_slice(&m.pp);
    b
}

/// Locate `TrainerNames` in a cartridge and decode it.
///
/// The table is a terminator-separated list in trainer-class order
/// ([`data/trainers/names.asm`](https://github.com/pret/pokered/blob/master/data/trainers/names.asm)),
/// so class *n* is the *n*-th entry, 1-based. It is found by its first four
/// entries rather than its first one, because `YOUNGSTER` alone also appears in
/// a second list in this cartridge — an ambiguous signature is not evidence.
fn trainer_class_names(rom: &[u8]) -> Vec<String> {
    let mut sig: Vec<u8> = Vec::new();
    for (i, name) in ["YOUNGSTER", "BUG CATCHER", "LASS", "SAILOR"]
        .iter()
        .enumerate()
    {
        if i > 0 {
            sig.push(text::TERMINATOR);
        }
        sig.extend(name.chars().filter_map(text::encode_char));
    }
    let mut at = None;
    let mut i = 0;
    while i + sig.len() <= rom.len() {
        if rom[i..i + sig.len()] == sig[..] {
            if at.is_some() {
                return Vec::new(); // ambiguous: refuse rather than guess
            }
            at = Some(i);
        }
        i += 1;
    }
    let Some(mut p) = at else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // Gen 1 has 47 trainer classes; stop at the first entry that does not look
    // like a name so a mislocated table cannot run away through the ROM.
    for _ in 0..47 {
        let start = p;
        while p < rom.len() && rom[p] != text::TERMINATOR {
            p += 1;
        }
        if p >= rom.len() || p == start {
            break;
        }
        let name = text::decode(&rom[start..p]);
        if name.is_empty() {
            break;
        }
        out.push(name);
        p += 1;
    }
    out
}

/// Fold a trainer class name to a comparison key. `♂`/`♀` become `m`/`f`
/// rather than being dropped, because `JR.TRAINER♂` and `JR.TRAINER♀` are two
/// different classes and would otherwise collide.
fn normalise_class(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\u{2642}' => Some('m'),
            '\u{2640}' => Some('f'),
            c if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

impl Plugin for BattlePlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capability {
        // WRITE_MEMORY is what makes the trigger possible and is why the status
        // bar marks this plugin `!`. CONTROL_INPUT is separate because pressing
        // buttons is something the game sanctions and a poke is not.
        Capability::OBSERVE
            | Capability::READ_MEMORY
            | Capability::WRITE_MEMORY
            | Capability::CONTROL_INPUT
    }

    fn status(&self) -> String {
        if let Some(r) = &self.refusal {
            return format!("inactive: {r}");
        }
        let sel = self
            .selected()
            .map(|p| p.name.as_str())
            .unwrap_or("<no presets>");
        match self.phase() {
            Phase::Idle => format!("{sel} ready ({} started)", self.battles_started),
            other => {
                if let Some((i, n)) = self.script_progress() {
                    format!("{sel}: {} [{i}/{n}]", other.label())
                } else {
                    format!("{sel}: {}", other.label())
                }
            }
        }
    }

    fn on_attach(&mut self, machine: &mut Machine) {
        let title = machine.rom_title();
        let sum = machine.global_checksum();
        match Game::detect(&title, sum) {
            Some(g) => {
                self.game = Some(g);
                self.log.push(format!("attached to {}", g.label()));
            }
            None => {
                let why = format!("{} (checksum ${sum:04X})", title.trim());
                self.refusal = Some(why.clone());
                self.log
                    .push(format!("refusing to write to an unknown cartridge: {why}"));
            }
        }
    }

    fn on_event(&mut self, event: &Event, machine: &mut Machine) {
        if self.refusal.is_some() || self.active.is_none() {
            return;
        }
        if matches!(event, Event::Frame { .. }) {
            self.on_frame(machine);
        }
    }

    fn reset(&mut self) {
        // Plugin state is not machine state. A save state loaded mid-battle
        // would leave this state machine describing a fight that is no longer
        // happening, and — worse — holding a restore snapshot of a party that
        // no longer exists. Drop both.
        if self.active.take().is_some() {
            self.log.push("reset (save state loaded)".into());
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests;

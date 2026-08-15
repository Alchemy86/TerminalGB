//! The state of a battle, laid out the way the cartridge lays it out.
//!
//! The shapes here mirror `wBattleMon`/`wEnemyMon` and their satellites rather
//! than an idealised design, because every one of them has to be filled from a
//! real machine by the differential harness and compared byte for byte. Where
//! the cartridge keeps two copies of something — the *modified* stat in
//! `wBattleMonAttack` and the *unmodified* one in `wPlayerMonUnmodifiedAttack` —
//! this keeps two copies too. Collapsing them would be a model that cannot
//! express a critical hit, which reads the unmodified pair.
//!
//! # Citations
//!
//! * `wBattleMon` / `wEnemyMon` — `ram/wram.asm`, the 28-byte battle struct.
//! * `wPlayerMonStatMods` / `wEnemyMonStatMods` — six bytes, **7 is neutral**,
//!   in the order attack, defense, speed, special, accuracy, evasion.
//! * `wPlayerBattleStatus1..3` / `wEnemyBattleStatus1..3` —
//!   `constants/battle_constants.asm`.
//! * Status bits — `constants/status_constants.asm`: `SLP` is a **counter** in
//!   bits 0-2, and `PSN`, `BRN`, `FRZ`, `PAR` are single bits 3-6.

use crate::data::{BaseStats, GameData, MoveData, STRUGGLE};
use crate::types::Type;

/// `MAX_STAT_VALUE` — `constants/battle_constants.asm`.
pub const MAX_STAT_VALUE: u16 = 999;
/// Neutral stat stage. The cartridge stores stages 1..=13 with 7 as neutral, so
/// "+1" on screen is a stored 8.
pub const NEUTRAL_STAGE: u8 = 7;
/// `MIN_STAT_MOD` / `MAX_STAT_MOD` — the stored range, not the −6..+6 a player
/// sees.
pub const MIN_STAGE: u8 = 1;
pub const MAX_STAGE: u8 = 13;

/// Non-volatile status, as the one byte the cartridge stores.
///
/// `constants/status_constants.asm`. Sleep is a **counter** in the low three
/// bits, which is why this is one byte and not a set of flags: a Pokémon cannot
/// be asleep and poisoned at once, and the representation is what enforces it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status(pub u8);

impl Status {
    pub const SLP_MASK: u8 = 0b0000_0111;
    pub const PSN: u8 = 1 << 3;
    pub const BRN: u8 = 1 << 4;
    pub const FRZ: u8 = 1 << 5;
    pub const PAR: u8 = 1 << 6;

    pub const HEALTHY: Status = Status(0);

    pub fn asleep(turns: u8) -> Status {
        Status(turns & Self::SLP_MASK)
    }
    pub fn sleep_turns(self) -> u8 {
        self.0 & Self::SLP_MASK
    }
    pub fn is_asleep(self) -> bool {
        self.sleep_turns() != 0
    }
    pub fn is_poisoned(self) -> bool {
        self.0 & Self::PSN != 0
    }
    pub fn is_burned(self) -> bool {
        self.0 & Self::BRN != 0
    }
    pub fn is_frozen(self) -> bool {
        self.0 & Self::FRZ != 0
    }
    pub fn is_paralysed(self) -> bool {
        self.0 & Self::PAR != 0
    }
    pub fn is_healthy(self) -> bool {
        self.0 == 0
    }
    /// Whether the Pokémon cannot act at all this turn purely from its
    /// non-volatile status. Paralysis is *not* here — it is a per-turn roll.
    pub fn immobilised(self) -> bool {
        self.is_asleep() || self.is_frozen()
    }
    pub fn label(self) -> &'static str {
        if self.is_asleep() {
            "SLP"
        } else if self.is_frozen() {
            "FRZ"
        } else if self.is_paralysed() {
            "PAR"
        } else if self.is_burned() {
            "BRN"
        } else if self.is_poisoned() {
            "PSN"
        } else {
            "OK"
        }
    }
}

/// The six stat stages, stored as the cartridge stores them (7 = neutral).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stages {
    pub attack: u8,
    pub defense: u8,
    pub speed: u8,
    pub special: u8,
    pub accuracy: u8,
    pub evasion: u8,
}

impl Default for Stages {
    fn default() -> Self {
        Stages {
            attack: NEUTRAL_STAGE,
            defense: NEUTRAL_STAGE,
            speed: NEUTRAL_STAGE,
            special: NEUTRAL_STAGE,
            accuracy: NEUTRAL_STAGE,
            evasion: NEUTRAL_STAGE,
        }
    }
}

/// Which stat a stage or a modifier refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stat {
    Attack,
    Defense,
    Speed,
    Special,
    Accuracy,
    Evasion,
}

impl Stages {
    pub fn get(&self, stat: Stat) -> u8 {
        match stat {
            Stat::Attack => self.attack,
            Stat::Defense => self.defense,
            Stat::Speed => self.speed,
            Stat::Special => self.special,
            Stat::Accuracy => self.accuracy,
            Stat::Evasion => self.evasion,
        }
    }
    fn slot(&mut self, stat: Stat) -> &mut u8 {
        match stat {
            Stat::Attack => &mut self.attack,
            Stat::Defense => &mut self.defense,
            Stat::Speed => &mut self.speed,
            Stat::Special => &mut self.special,
            Stat::Accuracy => &mut self.accuracy,
            Stat::Evasion => &mut self.evasion,
        }
    }
    /// Move a stage, saturating at the stored 1..=13. Returns whether it moved —
    /// a stage already at the cap produces the "won't go any higher" message and
    /// **still consumes the turn**.
    pub fn shift(&mut self, stat: Stat, by: i8) -> bool {
        let slot = self.slot(stat);
        let want = i16::from(*slot) + i16::from(by);
        let clamped = want.clamp(i16::from(MIN_STAGE), i16::from(MAX_STAGE)) as u8;
        let moved = clamped != *slot;
        *slot = clamped;
        moved
    }
    /// Reset every stage to neutral — Haze, and a switch-out.
    pub fn reset(&mut self) {
        *self = Stages::default();
    }
    /// Whether any stage is off neutral. `wPlayerMonStatMods` is zeroed WRAM
    /// until `InitBattleVariables` writes the 7s, so an all-zero block is *not*
    /// "−6 on everything" — see `plugins/trainer`, which learned this live.
    pub fn is_neutral(&self) -> bool {
        *self == Stages::default()
    }
    /// Whether the block is one the game wrote rather than uninitialised WRAM.
    pub fn is_valid(&self) -> bool {
        [
            self.attack,
            self.defense,
            self.speed,
            self.special,
            self.accuracy,
            self.evasion,
        ]
        .iter()
        .all(|&v| (MIN_STAGE..=MAX_STAGE).contains(&v))
    }
}

/// One of a Pokémon's four move slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MoveSlot {
    /// Move id, 0 for `NO_MOVE`.
    pub id: u8,
    /// Current PP, low six bits of the stored byte.
    pub pp: u8,
    /// PP Ups applied, the top two bits.
    pub pp_ups: u8,
}

impl MoveSlot {
    pub fn empty() -> MoveSlot {
        MoveSlot::default()
    }
    pub fn is_empty(&self) -> bool {
        self.id == 0
    }
    /// Decode the raw PP byte the cartridge stores: PP in bits 0-5, PP Ups in
    /// bits 6-7.
    pub fn from_raw(id: u8, raw_pp: u8) -> MoveSlot {
        MoveSlot {
            id,
            pp: raw_pp & 0x3F,
            pp_ups: raw_pp >> 6,
        }
    }
    pub fn raw_pp(&self) -> u8 {
        (self.pp & 0x3F) | (self.pp_ups << 6)
    }
}

/// The volatile flags a battle keeps, `wPlayerBattleStatus1..3`.
///
/// Named rather than bit-packed because the engine reads them constantly and a
/// wrong bit position is a silent behavioural change; the packed form is
/// reconstructed only when the harness compares against WRAM.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Volatile {
    /// `STORING_ENERGY` — Bide.
    pub bide: bool,
    /// `THRASHING_ABOUT` — Thrash/Petal Dance, and the confusion that follows.
    pub thrashing: bool,
    /// `ATTACKING_MULTIPLE_TIMES` — a multi-hit move in progress.
    pub multi_hit: bool,
    /// `FLINCHED`.
    pub flinched: bool,
    /// `CHARGING_UP` — Solarbeam, Sky Attack, Dig, Fly, Razor Wind, Skull Bash.
    pub charging: bool,
    /// `USING_TRAPPING_MOVE` — Wrap, Bind, Fire Spin, Clamp.
    pub trapping: bool,
    /// `INVULNERABLE` — the airborne/underground half of Fly and Dig.
    pub invulnerable: bool,
    /// `CONFUSED`.
    pub confused: bool,
    /// Turns of confusion left. Zero when `confused` is false.
    pub confusion_turns: u8,
    /// `PROTECTED_BY_MIST`.
    pub mist: bool,
    /// `GETTING_PUMPED` — Focus Energy. See [`crate::mechanics::critical_hit`]
    /// for what it actually does.
    pub focus_energy: bool,
    /// `HAS_SUBSTITUTE_UP`, with the substitute's remaining HP.
    pub substitute_hp: Option<u8>,
    /// `NEEDS_TO_RECHARGE` — Hyper Beam.
    pub recharging: bool,
    /// `HAS_REFLECT_UP`.
    pub reflect: bool,
    /// `HAS_LIGHT_SCREEN_UP`.
    pub light_screen: bool,
    /// `SEEDED` — Leech Seed.
    pub seeded: bool,
    /// `BADLY_POISONED` — Toxic, and its doubling counter.
    pub toxic: bool,
    /// `wPlayerToxicCounter`.
    pub toxic_counter: u8,
    /// Turns a locked move (Thrash, or a trapping move) has left.
    pub locked_turns: u8,
    /// The move id a locked/charging/recharging state is committed to.
    pub locked_move: u8,
    /// Damage accumulated by Bide.
    pub bide_damage: u16,
    /// Turns of Bide left.
    pub bide_turns: u8,
    /// `wPlayerDisabledMove` — the slot (1-based) and turns left.
    pub disabled: Option<(u8, u8)>,
}

impl Volatile {
    /// What `SwitchPlayerMon` clears — everything except the screens, which in
    /// Gen 1 belong to the *side* rather than the Pokémon and survive a switch.
    ///
    /// Reflect and Light Screen surviving a switch is a Gen 1 behaviour, not an
    /// oversight here; `wPlayerBattleStatus3`'s screen bits are cleared only by
    /// the end of the battle or by Haze.
    pub fn on_switch_out(&mut self) {
        let reflect = self.reflect;
        let light_screen = self.light_screen;
        *self = Volatile::default();
        self.reflect = reflect;
        self.light_screen = light_screen;
    }

    /// Whether this Pokémon is committed to a move and cannot be given a fresh
    /// choice this turn.
    pub fn is_locked(&self) -> bool {
        self.recharging
            || self.charging
            || self.thrashing
            || (self.trapping && self.locked_turns > 0)
    }
}

/// A Pokémon as it exists inside a battle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BattleMon {
    /// Internal index number, as WRAM and the wire carry it.
    pub species: u8,
    pub level: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub status: Status,
    /// The in-battle types. Held separately from the species' base types
    /// because Conversion overwrites them.
    pub type1: Type,
    pub type2: Type,
    pub moves: [MoveSlot; 4],
    /// The stats the cartridge computed at switch-in, before stages: the
    /// `wPlayerMonUnmodified*` block. A critical hit reads these.
    pub unmodified: Stats,
    /// The stats in `wBattleMonAttack` and friends — stages, badge boosts and
    /// burn/paralysis penalties already folded in. Damage reads these.
    pub current: Stats,
    /// **The stats a CRITICAL HIT reads**, which are not always
    /// [`BattleMon::unmodified`].
    ///
    /// For the player, `GetDamageVarsForPlayerAttack` reads `wPartyMon1Attack`
    /// straight out of the party struct, so the two are the same. For the
    /// **enemy**, `GetEnemyMonStat` (`engine/battle/core.asm`) does something
    /// else entirely outside a link battle: it calls `GetMonHeader` on
    /// `wEnemyMonSpecies` and **recomputes the stat from base stats, `wEnemyMonDVs`
    /// and `wEnemyMonLevel`, with ZERO stat experience** — the comment in the
    /// source is *"this base address makes CalcStat look in
    /// [wLoadedMonSpeedExp] for DVs"*, and the byte pair it points at holds only
    /// the DVs.
    ///
    /// For an ordinary wild Pokémon that is the same number, which is why this
    /// hid for as long as it did. It is **not** the same number for a trainer's
    /// Pokémon carrying stat experience, and it is not the same number in a
    /// battle a tool has patched. The cartridge oracle found it: an enemy
    /// critical hit dealt 17 where the model said 16, twice, in different
    /// battles, and nothing else explained it.
    ///
    /// `None` means "the same as `unmodified`", which is right for the player
    /// and for any Pokémon the model built itself.
    pub crit_stats: Option<Stats>,
    pub stages: Stages,
    pub volatile: Volatile,
}

/// The four battle stats. HP is not among them — it is not modifiable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    pub attack: u16,
    pub defense: u16,
    pub speed: u16,
    pub special: u16,
}

impl Stats {
    pub fn get(&self, stat: Stat) -> u16 {
        match stat {
            Stat::Attack => self.attack,
            Stat::Defense => self.defense,
            Stat::Speed => self.speed,
            Stat::Special => self.special,
            // Accuracy and evasion have no stat, only a stage.
            Stat::Accuracy | Stat::Evasion => 0,
        }
    }
    pub fn set(&mut self, stat: Stat, v: u16) {
        match stat {
            Stat::Attack => self.attack = v,
            Stat::Defense => self.defense = v,
            Stat::Speed => self.speed = v,
            Stat::Special => self.special = v,
            Stat::Accuracy | Stat::Evasion => {}
        }
    }
}

impl BattleMon {
    /// Build a Pokémon at full health from a species and a move list, computing
    /// its stats from base stats and DVs the way `CalcStats` does.
    ///
    /// `stat = ((base + DV) * 2 + ceil(sqrt(statexp))/4) * level / 100 + 5`,
    /// and HP adds `level + 5` more. Stat experience is taken as zero here,
    /// which is right for a freshly built test Pokémon and wrong for a real
    /// party — the harness fills a real one from WRAM instead of rebuilding it.
    pub fn build(
        data: &GameData,
        species: u8,
        level: u8,
        dvs: Dvs,
        move_ids: [u8; 4],
    ) -> Option<BattleMon> {
        let base = *data.base_stats_for_index(species)?;
        let stat = |b: u8, dv: u8| -> u16 {
            let v = (u32::from(b) + u32::from(dv)) * 2;
            ((v * u32::from(level)) / 100 + 5).min(u32::from(MAX_STAT_VALUE)) as u16
        };
        let hp = {
            let v = (u32::from(base.hp) + u32::from(dvs.hp())) * 2;
            ((v * u32::from(level)) / 100 + u32::from(level) + 10) as u16
        };
        let stats = Stats {
            attack: stat(base.attack, dvs.attack),
            defense: stat(base.defense, dvs.defense),
            speed: stat(base.speed, dvs.speed),
            special: stat(base.special, dvs.special),
        };
        let mut moves = [MoveSlot::empty(); 4];
        for (i, &id) in move_ids.iter().enumerate() {
            if id != 0 {
                let pp = data.move_data(id).map(|m| m.pp).unwrap_or(0);
                moves[i] = MoveSlot { id, pp, pp_ups: 0 };
            }
        }
        Some(BattleMon {
            species,
            level,
            hp,
            max_hp: hp,
            status: Status::HEALTHY,
            type1: base.type1,
            type2: base.type2,
            moves,
            unmodified: stats,
            current: stats,
            crit_stats: None,
            stages: Stages::default(),
            volatile: Volatile::default(),
        })
    }

    pub fn is_fainted(&self) -> bool {
        self.hp == 0
    }

    /// The stats a critical hit reads — [`BattleMon::crit_stats`] when the
    /// caller supplied them, otherwise the unmodified block.
    pub fn stats_for_crit(&self) -> &Stats {
        self.crit_stats.as_ref().unwrap_or(&self.unmodified)
    }

    /// The defender's type pair, with the second slot dropped when it repeats
    /// the first.
    pub fn types(&self) -> (Type, Option<Type>) {
        if self.type2 == self.type1 {
            (self.type1, None)
        } else {
            (self.type1, Some(self.type2))
        }
    }

    /// Whether a move slot can be selected: it exists and has PP.
    pub fn can_use(&self, slot: usize) -> bool {
        match self.moves.get(slot) {
            Some(m) => {
                !m.is_empty()
                    && m.pp > 0
                    && self
                        .volatile
                        .disabled
                        .map(|(s, _)| s as usize != slot + 1)
                        .unwrap_or(true)
            }
            None => false,
        }
    }

    /// The slots a fresh choice may pick from. Empty means Struggle.
    pub fn usable_slots(&self) -> Vec<usize> {
        (0..4).filter(|&i| self.can_use(i)).collect()
    }

    pub fn hp_fraction(&self) -> f32 {
        if self.max_hp == 0 {
            0.0
        } else {
            f32::from(self.hp) / f32::from(self.max_hp)
        }
    }
}

/// A Pokémon's four DVs. The HP DV is derived, never stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Dvs {
    pub attack: u8,
    pub defense: u8,
    pub speed: u8,
    pub special: u8,
}

impl Dvs {
    /// The maximum spread — every DV 15.
    pub const PERFECT: Dvs = Dvs {
        attack: 15,
        defense: 15,
        speed: 15,
        special: 15,
    };

    /// The HP DV is the low bit of each of the other four, most significant
    /// first: attack, defense, speed, special (`home/pokemon.asm`, `CalcStats`).
    pub fn hp(&self) -> u8 {
        ((self.attack & 1) << 3)
            | ((self.defense & 1) << 2)
            | ((self.speed & 1) << 1)
            | (self.special & 1)
    }

    /// Decode the two bytes WRAM stores: attack and defense in the first,
    /// speed and special in the second, each a nibble, high nibble first.
    pub fn from_bytes(hi: u8, lo: u8) -> Dvs {
        Dvs {
            attack: hi >> 4,
            defense: hi & 0x0F,
            speed: lo >> 4,
            special: lo & 0x0F,
        }
    }
}

/// One side's team.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Party {
    pub mons: Vec<BattleMon>,
    /// Index of the Pokémon currently out.
    pub active: usize,
}

impl Party {
    pub fn new(mons: Vec<BattleMon>) -> Party {
        Party { mons, active: 0 }
    }
    pub fn active(&self) -> &BattleMon {
        &self.mons[self.active]
    }
    pub fn active_mut(&mut self) -> &mut BattleMon {
        &mut self.mons[self.active]
    }
    /// Slots that could be switched to: alive, and not the one already out.
    pub fn switchable(&self) -> Vec<usize> {
        (0..self.mons.len())
            .filter(|&i| i != self.active && !self.mons[i].is_fainted())
            .collect()
    }
    pub fn all_fainted(&self) -> bool {
        self.mons.iter().all(BattleMon::is_fainted)
    }
    pub fn alive(&self) -> usize {
        self.mons.iter().filter(|m| !m.is_fainted()).count()
    }
}

/// Which side of the battle. The names are the cartridge's `hWhoseTurn`: 0 is
/// the player, non-zero is the enemy.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Side {
    Player,
    Enemy,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Player => Side::Enemy,
            Side::Enemy => Side::Player,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Side::Player => "player",
            Side::Enemy => "enemy",
        }
    }
}

/// What kind of battle this is — it changes what actions are legal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BattleKind {
    /// A wild encounter: running is allowed, and the opponent never switches.
    Wild,
    /// A trainer battle: no running, and the opponent switches when it faints.
    Trainer,
    /// A link battle. Badge boosts do **not** apply
    /// (`ApplyBadgeStatBoosts` returns early on `LINK_STATE_BATTLING`), and the
    /// dice come from a shared list rather than from `rDIV`.
    Link,
}

/// The whole battle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Battle {
    pub player: Party,
    pub enemy: Party,
    pub kind: BattleKind,
    /// `wObtainedBadges`. Only bits 0, 2, 4 and 6 matter, and only outside a
    /// link battle.
    pub badges: u8,
    /// Turns elapsed. Not a cartridge variable — a stopping condition, so an
    /// episode cannot run forever.
    pub turn: u32,
    /// The damage the last move dealt, which is what Counter strikes back.
    /// Shared by both sides, exactly as `wDamage` is — that sharing *is* the
    /// Counter bug.
    pub last_damage: u16,
    /// The move each side last used, for Counter's legality check and for Mirror
    /// Move.
    pub last_move: [u8; 2],
}

impl Battle {
    pub fn new(player: Party, enemy: Party, kind: BattleKind) -> Battle {
        Battle {
            player,
            enemy,
            kind,
            badges: 0,
            turn: 0,
            last_damage: 0,
            last_move: [0, 0],
        }
    }

    pub fn side(&self, side: Side) -> &Party {
        match side {
            Side::Player => &self.player,
            Side::Enemy => &self.enemy,
        }
    }
    pub fn side_mut(&mut self, side: Side) -> &mut Party {
        match side {
            Side::Player => &mut self.player,
            Side::Enemy => &mut self.enemy,
        }
    }
    pub fn active(&self, side: Side) -> &BattleMon {
        self.side(side).active()
    }
    pub fn active_mut(&mut self, side: Side) -> &mut BattleMon {
        self.side_mut(side).active_mut()
    }

    /// Who has won, if anyone.
    pub fn outcome(&self) -> Option<Outcome> {
        match (self.player.all_fainted(), self.enemy.all_fainted()) {
            (true, true) => Some(Outcome::Draw),
            (true, false) => Some(Outcome::EnemyWins),
            (false, true) => Some(Outcome::PlayerWins),
            (false, false) => None,
        }
    }

    /// Badge boosts do not apply in a link battle
    /// (`ApplyBadgeStatBoosts`, `engine/battle/core.asm`).
    pub fn badges_apply(&self) -> bool {
        self.kind != BattleKind::Link
    }
}

/// How a battle ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    PlayerWins,
    EnemyWins,
    /// Both sides wiped out on the same turn — reachable through recoil and
    /// through Explosion.
    Draw,
    /// The player ran from a wild battle.
    Fled,
    /// The turn limit was reached. Not a cartridge outcome; an episode's.
    TurnLimit,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::PlayerWins => "player wins",
            Outcome::EnemyWins => "enemy wins",
            Outcome::Draw => "draw",
            Outcome::Fled => "fled",
            Outcome::TurnLimit => "turn limit",
        }
    }
    pub fn is_win(self) -> bool {
        self == Outcome::PlayerWins
    }
}

/// What a side does on a turn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Action {
    /// Use the move in slot 0..=3.
    Move(usize),
    /// Switch to party slot n.
    Switch(usize),
    /// Run. Only legal in a wild battle.
    Run,
    /// Struggle — not chosen by a player, but the engine's answer when nothing
    /// else is usable.
    Struggle,
}

impl Action {
    pub fn move_id(self, mon: &BattleMon) -> u8 {
        match self {
            Action::Move(slot) => mon.moves.get(slot).map(|m| m.id).unwrap_or(0),
            Action::Struggle => STRUGGLE,
            _ => 0,
        }
    }
    pub fn is_switch(self) -> bool {
        matches!(self, Action::Switch(_))
    }
    pub fn describe(self, mon: &BattleMon, data: &GameData) -> String {
        match self {
            Action::Move(slot) => {
                let id = mon.moves.get(slot).map(|m| m.id).unwrap_or(0);
                match data.move_name(id) {
                    Some(n) => n.to_string(),
                    None => format!("move #{id}"),
                }
            }
            Action::Switch(i) => format!("switch to slot {}", i + 1),
            Action::Run => "run".into(),
            Action::Struggle => "STRUGGLE".into(),
        }
    }
}

/// A convenience view of a move a side is about to use, resolved against the
/// data tables.
#[derive(Clone, Copy, Debug)]
pub struct ChosenMove {
    pub data: MoveData,
    /// The slot it came from, or `None` for Struggle.
    pub slot: Option<usize>,
}

/// Compute a stat the way `CalcStats` (`home/pokemon.asm`) does, for building a
/// Pokémon outside a cartridge.
pub fn calc_stat(base: u8, dv: u8, stat_exp: u16, level: u8, is_hp: bool) -> u16 {
    // The stat-experience term is `ceil(sqrt(statexp)) / 4`, capped at 63.
    let exp_term = {
        let mut root = 0u32;
        while (root + 1) * (root + 1) <= u32::from(stat_exp) {
            root += 1;
        }
        if u32::from(stat_exp) > root * root {
            root += 1;
        }
        (root / 4).min(63)
    };
    let inner = (u32::from(base) + u32::from(dv)) * 2 + exp_term;
    let base_term = inner * u32::from(level) / 100;
    let v = if is_hp {
        base_term + u32::from(level) + 10
    } else {
        base_term + 5
    };
    v.min(u32::from(MAX_STAT_VALUE)) as u16
}

/// Recompute a species' base stats view for a `BattleMon`. Convenience for
/// agents and reports.
pub fn base_of<'a>(data: &'a GameData, mon: &BattleMon) -> Option<&'a BaseStats> {
    data.base_stats_for_index(mon.species)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_is_a_counter_and_excludes_the_other_statuses() {
        let s = Status::asleep(3);
        assert_eq!(s.sleep_turns(), 3);
        assert!(s.is_asleep());
        assert!(!s.is_poisoned());
        assert!(s.immobilised());
        assert_eq!(s.label(), "SLP");
    }

    /// Paralysis does not stop a turn on its own — it is a per-turn roll, so it
    /// must not be counted as immobilising state.
    #[test]
    fn paralysis_is_not_immobilising_state() {
        assert!(!Status(Status::PAR).immobilised());
        assert!(Status(Status::FRZ).immobilised());
    }

    #[test]
    fn stages_are_stored_with_seven_as_neutral_and_saturate() {
        let mut s = Stages::default();
        assert!(s.is_neutral());
        assert!(s.shift(Stat::Attack, 2));
        assert_eq!(s.attack, 9);
        assert!(s.shift(Stat::Attack, 6)); // clamps at 13
        assert_eq!(s.attack, MAX_STAGE);
        // Already at the cap: no movement, and the caller must still spend the turn.
        assert!(!s.shift(Stat::Attack, 1));
        assert!(s.shift(Stat::Attack, -20));
        assert_eq!(s.attack, MIN_STAGE);
    }

    /// An all-zero stat-mod block is uninitialised WRAM, not "−6 on
    /// everything". `plugins/trainer` found this live, printing
    /// `ATK-6 DEF-6 SPD-6 SPC-6` for a Pokémon nothing had touched.
    #[test]
    fn an_all_zero_stage_block_is_not_valid() {
        let zeroed = Stages {
            attack: 0,
            defense: 0,
            speed: 0,
            special: 0,
            accuracy: 0,
            evasion: 0,
        };
        assert!(!zeroed.is_valid());
        assert!(Stages::default().is_valid());
    }

    /// The HP DV is the low bit of each of the other four, most significant
    /// first.
    #[test]
    fn the_hp_dv_is_derived_from_the_other_four() {
        assert_eq!(Dvs::PERFECT.hp(), 15);
        assert_eq!(
            Dvs {
                attack: 0,
                defense: 0,
                speed: 0,
                special: 0
            }
            .hp(),
            0
        );
        assert_eq!(
            Dvs {
                attack: 1,
                defense: 0,
                speed: 0,
                special: 0
            }
            .hp(),
            8
        );
        assert_eq!(
            Dvs {
                attack: 0,
                defense: 0,
                speed: 0,
                special: 1
            }
            .hp(),
            1
        );
        // The spread that becomes shiny in Gen 2: def/spd/spc all 10, attack
        // with bit 1 set.
        let shiny = Dvs {
            attack: 10,
            defense: 10,
            speed: 10,
            special: 10,
        };
        assert_eq!(shiny.hp(), 0);
    }

    #[test]
    fn dvs_decode_from_the_two_stored_bytes() {
        assert_eq!(
            Dvs::from_bytes(0x6B, 0x3D),
            Dvs {
                attack: 6,
                defense: 11,
                speed: 3,
                special: 13
            }
        );
    }

    #[test]
    fn a_pp_byte_carries_pp_ups_in_its_top_two_bits() {
        let m = MoveSlot::from_raw(85, 0b1100_1111);
        assert_eq!(m.pp, 15);
        assert_eq!(m.pp_ups, 3);
        assert_eq!(m.raw_pp(), 0b1100_1111);
    }

    /// Reflect and Light Screen belong to the side in Gen 1 and survive a
    /// switch; everything else on the Pokémon does not.
    #[test]
    fn a_switch_clears_the_volatiles_but_not_the_screens() {
        let mut v = Volatile {
            confused: true,
            confusion_turns: 3,
            reflect: true,
            light_screen: true,
            substitute_hp: Some(20),
            ..Volatile::default()
        };
        v.on_switch_out();
        assert!(!v.confused);
        assert_eq!(v.confusion_turns, 0);
        assert_eq!(v.substitute_hp, None);
        assert!(v.reflect);
        assert!(v.light_screen);
    }

    /// `CalcStats` at level 100 with perfect DVs and no stat experience.
    #[test]
    fn calc_stat_matches_the_cartridge_shape() {
        // Level 100, base 100, DV 15: (100+15)*2 * 100/100 + 5 = 235.
        assert_eq!(calc_stat(100, 15, 0, 100, false), 235);
        // HP adds level + 5 more: 230 + 100 + 10 = 340.
        assert_eq!(calc_stat(100, 15, 0, 100, true), 340);
        // The 999 cap is UNREACHABLE from a legal Pokémon: the largest possible
        // input — base 255, DV 15, maxed stat experience, level 100 — is 608.
        // A stat only reaches 999 through stat stages, which is why the cap
        // lives in `CalculateModifiedStat` and not here.
        assert_eq!(calc_stat(255, 15, 65535, 100, false), 608);
        assert!(608 < MAX_STAT_VALUE);
    }

    #[test]
    fn an_outcome_is_only_reported_when_a_side_is_wiped_out() {
        let mon = |hp: u16| BattleMon {
            species: 1,
            level: 5,
            hp,
            max_hp: 20,
            status: Status::HEALTHY,
            type1: Type::NORMAL,
            type2: Type::NORMAL,
            moves: [MoveSlot::empty(); 4],
            unmodified: Stats::default(),
            current: Stats::default(),
            crit_stats: None,
            stages: Stages::default(),
            volatile: Volatile::default(),
        };
        let b = Battle::new(
            Party::new(vec![mon(10)]),
            Party::new(vec![mon(10)]),
            BattleKind::Wild,
        );
        assert_eq!(b.outcome(), None);
        let b = Battle::new(
            Party::new(vec![mon(10)]),
            Party::new(vec![mon(0)]),
            BattleKind::Wild,
        );
        assert_eq!(b.outcome(), Some(Outcome::PlayerWins));
        let b = Battle::new(
            Party::new(vec![mon(0)]),
            Party::new(vec![mon(0)]),
            BattleKind::Wild,
        );
        assert_eq!(b.outcome(), Some(Outcome::Draw));
    }

    #[test]
    fn badges_do_not_apply_in_a_link_battle() {
        let mons = || Party::new(vec![]);
        assert!(!Battle::new(mons(), mons(), BattleKind::Link).badges_apply());
        assert!(Battle::new(mons(), mons(), BattleKind::Trainer).badges_apply());
    }
}

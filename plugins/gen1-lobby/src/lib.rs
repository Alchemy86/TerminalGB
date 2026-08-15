//! # The World Trade Centre — a shared room many players walk about in
//!
//! A Gen 1 Cable Club Trade Center, four screens by three instead of one, with
//! a floor drawn in this repository, holding everyone the relay says is in the
//! room — each of them walking under their own control, on their own console,
//! in their own process.
//!
//! Nothing is patched, nothing is distributed and no save is touched. The room
//! is a block map written into unsaved WRAM at run time; the avatars are sprite
//! structs written into unsaved WRAM at run time; the walking, the collision,
//! the camera, the walk cycle and the bump sound are all the cartridge's,
//! operating on data we supplied.
//!
//! ## What this plugin is and is not
//!
//! It is **presence**: the room, the crowd, and what people say. It is
//! deliberately not the network — see §"Where the network lives" — and it is
//! not the trade. A trade or a battle is still strictly 1:1 over the link
//! cable, exactly as it works today; the lobby's job is to be the place two
//! people meet before they start one.
//!
//! ```no_run
//! # use terminalgb_plugin_gen1_lobby::{LobbyPlugin, avatar::Peer};
//! let mut lobby = LobbyPlugin::new("RED");
//! // …attached to a `Gameboy`, then once per relay tick:
//! lobby.set_peers(vec![Peer::new(2, "BLUE", 12, 9)]);
//! let me = lobby.me();          // -> back onto the relay
//! ```
//!
//! ## Where the network lives
//!
//! Not here. A [`Plugin`] is handed a [`Machine`] *during a frame the emulator
//! has already decided to run*; a socket is not that, and a plugin that blocked
//! on one would be the audio hand-off bug again (`docs/audio.md` §4c). So this
//! crate is pure memory work with no dependency but the plugin API, and the
//! relay client lives in the frontend, pushing snapshots in with
//! [`LobbyPlugin::set_peers`] and reading [`LobbyPlugin::me`] back out. That
//! also makes every rule in here testable at unit speed with no ROM, no
//! network and no cartridge.
//!
//! ## Safety
//!
//! Two rules, both enforced rather than promised:
//!
//! 1. **Nothing happens outside the Cable Club.** Every write is gated on
//!    `wCurMap` being the Trade Center or the Colosseum *and* `wLinkState`
//!    being `LINK_STATE_IN_CABLE_CLUB`. That is the state in which `LinkMenu`
//!    has set `BIT_LINK_CONNECTED`, so `DrawStartMenu` prints **RESET** where
//!    SAVE would be and `StartMenu_SaveReset` jumps to `Init` — `SaveMainData`
//!    *cannot run*. The five saved-region bytes the room needs (two dimension
//!    bytes, two view-pointer bytes) therefore cannot reach a `.sav`.
//! 2. **We stop the moment the game wants its buffers back.** At
//!    `wLinkState >= LINK_STATE_START_TRADE` the trade screen owns everything,
//!    so the plugin does nothing at all until it is back in the room — and
//!    `ReturnToCableClubRoom` restores the cartridge's own 5x4 header from ROM,
//!    which the plugin notices and re-installs from.
//!
//! Everything else — `wOverworldMap`, `wSpriteStateData1/2`, `wTileMap` and
//! `vTileset` — is below `wMainDataStart` or is not WRAM at all, and can never
//! be written to a save file by any path.

#![forbid(unsafe_code)]

pub mod art;
pub mod avatar;
pub mod bubble;
pub mod gen1;
pub mod room;
pub mod screen;

use art::Floor;
use avatar::{Appearance, Peer, Presence};
use bubble::Speech;
use gen1::*;
use room::{Room, DEFAULT_ROOM};
use screen::Blockset;

use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};

/// What the plugin is doing, for the status line and for a host that wants to
/// know whether to bother sending position updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    /// Not in a Cable Club room. Nothing is written.
    #[default]
    Outside,
    /// In the room, room installed, avatars streaming.
    InRoom,
    /// A trade or battle screen is up. The game owns every buffer; we are
    /// deliberately idle.
    Busy,
}

/// The plugin. See the module docs.
pub struct LobbyPlugin {
    name: String,
    room: Room,
    peers: Vec<Peer>,
    state: State,
    me: Presence,
    look: Appearance,
    blockset: Option<Blockset>,
    blocks: Vec<u8>,
    /// The screen anchor row, measured against the engine before we paint.
    /// `None` means "we could not prove we agree with the engine", and the
    /// plugin then never writes `wTileMap` at all.
    anchor: Option<i32>,
    /// What that measurement said, for the report: `[(ok, total); 2]` over
    /// [`screen::ANCHOR_CANDIDATES`].
    pub calibration: [(usize, usize); 2],
    /// Screen rows a bubble borrowed last frame and owes back.
    borrowed: Vec<usize>,
    /// What the local player is saying, and for how many more frames.
    saying: Option<(String, u32)>,
    /// Bookkeeping the status line and the report are made of.
    pub installs: u32,
    pub art_installs: u32,
    pub ticks: u64,
    drawn: Vec<u32>,
    wrote_saved_region: bool,
}

impl LobbyPlugin {
    pub fn new(name: &str) -> LobbyPlugin {
        LobbyPlugin::with_room(name, DEFAULT_ROOM)
    }

    pub fn with_room(name: &str, room: Room) -> LobbyPlugin {
        LobbyPlugin {
            name: filter_to_charmap(name),
            room,
            peers: Vec::new(),
            state: State::Outside,
            me: Presence::default(),
            look: Appearance::default(),
            blockset: None,
            blocks: room.blocks(),
            anchor: None,
            calibration: [(0, 0); 2],
            borrowed: Vec::new(),
            saying: None,
            installs: 0,
            art_installs: 0,
            ticks: 0,
            drawn: Vec::new(),
            wrote_saved_region: false,
        }
    }

    /// The room this plugin installs.
    pub fn room(&self) -> Room {
        self.room
    }

    /// Where the local player is, as of the last frame in the room. This is
    /// what the host puts on the relay.
    pub fn me(&self) -> Presence {
        self.me
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Who is currently a game object, nearest first.
    pub fn drawn(&self) -> &[u32] {
        &self.drawn
    }

    /// Replace the crowd. Called once per relay tick — 8 Hz is twice what a
    /// 268 ms Gen 1 walk step needs, and a late packet costs one stale frame of
    /// somebody else's avatar, never a desync.
    pub fn set_peers(&mut self, peers: Vec<Peer>) {
        self.peers = peers;
    }

    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// Say one of [`bubble::PHRASES`], or anything else the charmap can draw.
    pub fn say(&mut self, text: &str) {
        let filtered = filter_to_charmap(text);
        if !filtered.trim().is_empty() {
            self.saying = Some((filtered, bubble::BUBBLE_FRAMES));
        }
    }

    /// What the local player is saying, for the relay to carry.
    pub fn saying(&self) -> Option<&str> {
        self.saying.as_ref().map(|(t, _)| t.as_str())
    }

    /// Has this plugin ever written a byte at or above `wMainDataStart`? The
    /// room's five header bytes are the only ones that can, and only inside the
    /// Cable Club — this is how a report says so with evidence rather than
    /// assertion.
    pub fn touched_saved_region(&self) -> bool {
        self.wrote_saved_region
    }

    /// The screen anchor the calibration settled on, or `None` if it could not
    /// reproduce the engine's own picture and the plugin therefore never
    /// touched `wTileMap`.
    pub fn anchor(&self) -> Option<i32> {
        self.anchor
    }

    fn in_a_cable_club_room(m: &mut Machine<'_>) -> Option<u8> {
        let map = m.peek(W_CUR_MAP)?;
        let link = m.peek(W_LINK_STATE)?;
        if (map == MAP_TRADE_CENTER || map == MAP_COLOSSEUM)
            && link == LINK_STATE_IN_CABLE_CLUB
        {
            Some(map)
        } else {
            None
        }
    }

    fn tick(&mut self, m: &mut Machine<'_>) {
        // Rule 2 first: if the trade screen is up we do nothing whatsoever.
        let link = m.peek(W_LINK_STATE).unwrap_or(0);
        if link >= LINK_STATE_START_TRADE {
            self.state = State::Busy;
            self.borrowed.clear();
            return;
        }
        let Some(_map) = Self::in_a_cable_club_room(m) else {
            self.state = State::Outside;
            self.borrowed.clear();
            return;
        };
        self.ticks += 1;

        // The blockset is the cartridge's and never moves; read it once.
        if self.blockset.is_none() {
            self.blockset = Blockset::read(m);
        }
        // Calibrate against the engine while the room is still the
        // cartridge's, which is the only moment there is an independent answer
        // to check against.
        //
        // **Retried every tick until it succeeds, and NOTHING is written until
        // it does.** `wCurMap` says Trade Center about thirty frames before
        // `LoadMapHeader` has run, and installing the room in that window draws
        // its blocks through the previous map's header — see
        // `docs/worldlobby.md` §9.1 for the measurement. This calibration is
        // the only test that asks the *engine* rather than a byte.
        if self.anchor.is_none() {
            if let Some(bs) = &self.blockset {
                let (best, scores) = screen::calibrate(m, bs);
                self.anchor = best;
                self.calibration = scores;
            }
        }
        if self.anchor.is_none() {
            // The two map bytes say Cable Club and the engine disagrees. We are
            // not in a room yet whatever `wCurMap` claims — say so, and touch
            // nothing.
            self.state = State::Outside;
            self.borrowed.clear();
            return;
        }
        self.state = State::InRoom;
        // Wear whatever the cartridge's own opponent object wears, so the
        // avatars cost no VRAM pattern slot of their own.
        if self.installs == 0 {
            self.look = Appearance::from_slot1(m);
        }

        // Install, or re-install after `ReturnToCableClubRoom` /
        // `ReloadMapData` put the cartridge's own room back.
        if !self.room.is_installed(m) && self.room.install(m).is_ok() {
            self.installs += 1;
            self.wrote_saved_region = true;
            // **Paint the view once, here and nowhere else.** The engine
            // redraws a row or a column only as it *scrolls in*, so the parts
            // of the screen that were the cartridge's three-block border a
            // moment ago would keep showing it until the player happened to
            // walk past them — the room would appear to have a dark edge that
            // is not there. One paint fixes it, and after this the engine is
            // the only writer of the screen buffer again, which is the whole
            // reason the room is a block map rather than a per-frame repaint.
            if let (Some(bs), Some(a)) = (&self.blockset, self.anchor) {
                let rows: Vec<usize> = (0..SCREEN_H).collect();
                screen::repaint_rows(m, &self.room, &self.blocks, bs, &rows, a);
            }
        }
        if !Floor::is_installed(m) {
            Floor::install(m);
            self.art_installs += 1;
        }

        self.me = Presence::read(m);
        self.drawn = avatar::stream(m, &self.look, &self.me, &self.peers);

        // Speech: mine plus everybody else's, but only for people who are
        // actually on screen — a bubble from someone you cannot see is a
        // bubble over nobody.
        let mut speech: Vec<Speech> = Vec::new();
        if let Some((text, left)) = &mut self.saying {
            speech.push(Speech {
                text: text.clone(),
                x: self.me.x,
                y: self.me.y,
            });
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.saying = None;
            }
        }
        for id in &self.drawn {
            if let Some(p) = self.peers.iter().find(|p| p.id == *id) {
                if let Some(t) = &p.says {
                    speech.push(Speech {
                        text: t.clone(),
                        x: p.x,
                        y: p.y,
                    });
                }
            }
        }
        // **Draw, then hand back only what is no longer wanted — never the
        // other way round.** `Plugin::on_event(Frame)` is dispatched from
        // `Gameboy::end_frame`, i.e. *after* the PPU has produced the picture,
        // so a write here is seen on the next frame. Repainting the bubble's
        // rows at the top of the tick and redrawing them at the bottom
        // therefore brackets the render exactly, and the bubble is never on
        // screen for a single frame — which is what it did, and it looked like
        // the text was not being written at all.
        let rows = bubble::draw(m, &speech);
        let stale: Vec<usize> = self
            .borrowed
            .iter()
            .copied()
            .filter(|r| !rows.contains(r))
            .collect();
        if !stale.is_empty() {
            if let (Some(bs), Some(a)) = (&self.blockset, self.anchor) {
                screen::repaint_rows(m, &self.room, &self.blocks, bs, &stale, a);
            }
        }
        self.borrowed = rows;
    }
}

impl Plugin for LobbyPlugin {
    fn name(&self) -> &str {
        "gen1-lobby"
    }

    fn capabilities(&self) -> Capability {
        // No SERIAL_LINK: the cable belongs to whoever the player is trading
        // with, and a lobby that quietly took it would be the reason a trade
        // stopped working. No CONTROL_INPUT: the player walks, we do not.
        Capability::OBSERVE | Capability::READ_MEMORY | Capability::WRITE_MEMORY
    }

    fn status(&self) -> String {
        match self.state {
            State::Outside => format!("{} outside", self.name),
            State::Busy => format!("{} trading", self.name),
            State::InRoom => format!(
                "{} ({},{}) {} of {} seen",
                self.name,
                self.me.x,
                self.me.y,
                self.drawn.len(),
                self.peers.len()
            ),
        }
    }

    fn on_event(&mut self, event: &Event, m: &mut Machine<'_>) {
        if let Event::Frame { .. } = event {
            self.tick(m);
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
mod tests {
    use super::*;

    #[test]
    fn the_plugin_never_asks_for_the_cable_or_the_buttons() {
        let p = LobbyPlugin::new("RED");
        let c = p.capabilities();
        assert!(c.contains(Capability::WRITE_MEMORY));
        assert!(!c.contains(Capability::SERIAL_LINK));
        assert!(!c.contains(Capability::CONTROL_INPUT));
    }

    #[test]
    fn a_name_is_filtered_to_what_the_cartridge_can_draw() {
        // A name goes on the wire and then onto somebody else's screen; a
        // glyph the font does not have would be drawn as whatever happens to
        // live at that tile id.
        let p = LobbyPlugin::new("R@D#1");
        assert_eq!(p.name, "RD1");
    }

    #[test]
    fn saying_nothing_drawable_says_nothing() {
        let mut p = LobbyPlugin::new("RED");
        p.say("@@@ ###");
        assert_eq!(p.saying(), None);
        p.say("HI!");
        assert_eq!(p.saying(), Some("HI!"));
    }

    #[test]
    fn the_default_room_is_the_one_the_design_recommends() {
        let p = LobbyPlugin::new("RED");
        assert_eq!(p.room(), Room::new(20, 14));
        assert_eq!(p.room().walk_size(), (40, 28));
    }
}

//! The Generation 1 link-cable protocol, from the point of view of the *other*
//! Game Boy.
//!
//! # Who drives the clock — and why that is the whole trick
//!
//! The Game Boy link is a **clocked synchronous** exchange: one console supplies
//! the clock and both shift a byte simultaneously. Every prior art that bridges
//! a real Game Boy to something else (a microcontroller, a browser, a network)
//! has to meet that clock in real time, and that is the hard part of the problem.
//!
//! An in-process virtual partner does not have that problem *if it arranges to
//! be the slave*, and Gen 1 lets us choose. `pret/pokered`'s
//! [`CableClubNPC`](https://github.com/pret/pokered/tree/master/engine/link/cable_club_npc.asm)
//! alternates two offers in a loop: it arms an externally-clocked transfer with
//! `$02` in `SB` ("you take the clock"), then immediately an internally-clocked
//! one sending `$01` ("I'll take the clock"). Its interrupt handler stores
//! whatever it *received* as the connection status, and the constants are chosen
//! so the two meanings coincide:
//!
//! ```text
//! ESTABLISH_CONNECTION_WITH_INTERNAL_CLOCK = $01     USING_EXTERNAL_CLOCK = $01
//! ESTABLISH_CONNECTION_WITH_EXTERNAL_CLOCK = $02     USING_INTERNAL_CLOCK = $02
//! ```
//!
//! So **replying `$02` to the game's `$01` makes the game the master for the
//! whole session**. Every later transfer is then started by the game writing
//! `SC = $81`, which this emulator already services synchronously — the partner
//! answers inside that write and has no timing budget to miss. That is why this
//! plugin needs no timing tolerance, no buffering and no clock recovery, and it
//! is the one design decision that makes a local virtual partner easy where a
//! hardware or network one is hard.
//!
//! # The exchange, in order
//!
//! 1. **Handshake** — game sends `$01`, we answer `$02`.
//! 2. **Nybble sync** — `Serial_SyncAndExchangeNybble` exchanges `$60 | n`
//!    values, then ten `$00` bytes. Used at every rendezvous point.
//! 3. **Link menu** — `$D0 | item | (button << 2)`; `$D4` is "A on TRADE CENTER".
//! 4. **Section exchange** — three preamble-delimited blocks (random numbers,
//!    party data, patch list). See [`super::party`].
//! 5. **Trade selection** — a nybble round carrying each side's chosen party
//!    slot (`$6F` = cancel).
//! 6. **Confirmation** — a nybble round: `1` cancel, `2` confirm.
//! 7. **Post-trade sync**, then back to step 4 for the next trade.
//!
//! # The re-sync byte
//!
//! `Serial_ExchangeBytes` starts each block with `hSerialIgnoringInitialData`
//! set and **re-sends its first byte** until it receives a preamble byte, only
//! then storing what follows. So a block of `N` bytes costs `N + 1` transfers
//! and our stream must lead each block with one extra `$FD`. Getting this off by
//! one shifts the entire party block and the game receives garbage — it is the
//! single easiest thing to get wrong here.

#[cfg(test)]
use super::battle::NoHost;
use super::battle::{self, Action, Host};
use super::party::{
    self, Mon, Party, PARTY_BLOCK_LEN, PARTY_PREAMBLE, PATCH_BLOCK_LEN, PATCH_PREAMBLE,
    PREAMBLE_BYTE, RANDOM_BLOCK_LEN, RANDOM_PREAMBLE,
};
use super::text::{self, NAME_LENGTH};

/// `ESTABLISH_CONNECTION_WITH_INTERNAL_CLOCK` — "I will drive the clock".
const ESTABLISH_INTERNAL: u8 = 0x01;
/// `ESTABLISH_CONNECTION_WITH_EXTERNAL_CLOCK` — "you drive the clock".
const ESTABLISH_EXTERNAL: u8 = 0x02;
/// Nybble-exchange values are `$60 | n`.
const NYBBLE_BASE: u8 = 0x60;
/// Link-menu selections are `$D0 | item | (button << 2)`.
const MENU_BASE: u8 = 0xD0;
/// The nybble meaning "cancel / back out".
const NYBBLE_CANCEL: u8 = 0x0F;
/// The whole wire byte for that nybble, `$6F` — which is what both sides
/// actually put on the link, and what the game waits to see come back before it
/// will let the player out of the trade screen. See [`LinkPartner::feed_cancel`].
const CANCEL_BYTE: u8 = NYBBLE_BASE | NYBBLE_CANCEL;
/// Confirmation nybbles.
const NYBBLE_DECLINE: u8 = 0x01;
const NYBBLE_ACCEPT: u8 = 0x02;

/// Total transfers in one full section exchange: each block costs its own
/// length plus one re-sync byte.
pub const SECTION_STREAM_LEN: usize = (1 + RANDOM_PREAMBLE + RANDOM_BLOCK_LEN)
    + (1 + PARTY_PREAMBLE + PARTY_BLOCK_LEN)
    + (1 + PATCH_PREAMBLE + PATCH_BLOCK_LEN);

/// Where the exchange currently is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing connected; waiting for the game to offer the clock.
    Handshake,
    /// Connected, before (or between) trades: receptionist, link menu, walking
    /// to the trade machine.
    Connected,
    /// Streaming the three blocks.
    Exchanging,
    /// A nybble round is choosing which Pokémon each side offers.
    SelectMon,
    /// A nybble round is confirming or cancelling.
    Confirm,
    /// The trade happened; one more sync round follows.
    PostTrade,
    /// **A link battle is running** and each nybble round is a battle turn.
    /// Reached only in the Colosseum; see [`super::battle`].
    Battling,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Handshake => "waiting for link",
            Phase::Connected => "connected",
            Phase::Exchanging => "exchanging party data",
            Phase::SelectMon => "choosing Pokémon",
            Phase::Confirm => "confirming trade",
            Phase::PostTrade => "trade complete",
            Phase::Battling => "link battle",
        }
    }
}

/// Which Cable Club room the player chose, as far as the partner can tell from
/// the link menu.
///
/// This is only a *fallback*: `wLinkState` is what the game itself acts on and
/// is preferred wherever it can be read. It is worth keeping because the link
/// menu is the one place the room crosses the cable at all — the game otherwise
/// never tells its partner which room it is in, which is exactly how a
/// trade-only partner ended up in a battle.
///
/// `LinkMenu` (`engine/menus/main_menu.asm`) exchanges `$D0 | item |
/// (button << 2)`; item 0 is TRADE CENTER, 1 COLOSSEUM, 2 CANCEL. When both
/// consoles press a button the one driving the clock wins, and that is the
/// game, so the item the player chose is always the item that happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Room {
    Unknown,
    TradeCenter,
    Colosseum,
    Cancelled,
}

/// The virtual second Game Boy.
///
/// Pure: it holds no emulator handle and does no I/O. `feed` maps one byte the
/// game sent to the byte this partner shifts back, which makes the whole
/// protocol testable without running a ROM.
pub struct LinkPartner {
    phase: Phase,
    /// The 644-byte stream we send during a section exchange.
    stream: Vec<u8>,
    stream_pos: usize,
    /// What the game sent during the exchange — this is how we learn what it
    /// offered, and it is the evidence a trade really happened.
    received: Vec<u8>,
    /// Which of our party slots we offer.
    offer_index: u8,
    /// True while inside a run of `$6x` bytes, so consecutive rounds are told
    /// apart by the `$00` bytes that always separate them.
    in_nybble_round: bool,
    /// The reply held constant for the duration of the current nybble round.
    nybble_reply: u8,
    /// The party this partner offers.
    party: Party,
    /// The last party the game offered us, decoded.
    seen_party: Option<Party>,
    /// The Pokémon the game gave up, once a trade is confirmed.
    traded_away: Option<Mon>,
    trades_completed: u32,
    exchanges_completed: u32,
    /// Which room the link menu selected, as a fallback for `wLinkState`.
    room: Room,
    /// The action taken on the most recent battle turn, for the status bar.
    last_action: Option<Action>,
    battle_turns: u32,
    /// Move choice is deliberately unpredictable — the opponent's behaviour
    /// used to be a pure function of the player's own button presses. Seeded
    /// from the offered party so a run is still reproducible.
    rng: u32,
}

impl LinkPartner {
    pub fn new(party: Party) -> Result<LinkPartner, party::PartyError> {
        let (block, patch) = party.to_blocks()?;
        let stream = build_stream(&block, &patch);
        debug_assert_eq!(stream.len(), SECTION_STREAM_LEN);
        let seed = party
            .mons
            .iter()
            .fold(0x9E37_79B9u32, |acc, m| {
                acc.rotate_left(7) ^ u32::from(m.species) ^ (u32::from(m.ot_id) << 8)
            })
            .max(1);
        Ok(LinkPartner {
            room: Room::Unknown,
            last_action: None,
            battle_turns: 0,
            rng: seed,
            phase: Phase::Handshake,
            stream,
            stream_pos: 0,
            received: Vec::with_capacity(SECTION_STREAM_LEN),
            offer_index: 0,
            in_nybble_round: false,
            nybble_reply: NYBBLE_BASE,
            party,
            seen_party: None,
            traded_away: None,
            trades_completed: 0,
            exchanges_completed: 0,
        })
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn trades_completed(&self) -> u32 {
        self.trades_completed
    }

    /// How many full three-block exchanges have finished. A useful milestone in
    /// its own right: it means the handshake, the menu and the party transfer
    /// all worked, even if no trade was confirmed.
    pub fn exchanges_completed(&self) -> u32 {
        self.exchanges_completed
    }

    /// The party the game most recently sent us.
    pub fn seen_party(&self) -> Option<&Party> {
        self.seen_party.as_ref()
    }

    /// What the player traded away, if a trade was confirmed.
    pub fn traded_away(&self) -> Option<&Mon> {
        self.traded_away.as_ref()
    }

    /// What this partner is offering.
    pub fn offered(&self) -> Option<&Mon> {
        self.party.mons.get(self.offer_index as usize)
    }

    /// Which Cable Club room the link menu chose.
    pub fn room(&self) -> Room {
        self.room
    }

    /// How many battle turns this partner has answered.
    pub fn battle_turns(&self) -> u32 {
        self.battle_turns
    }

    /// What it did on the most recent one.
    pub fn last_action(&self) -> Option<Action> {
        self.last_action
    }

    /// Drop all protocol state and go back to waiting for a handshake.
    pub fn reset(&mut self) {
        self.clear_scratch();
        self.phase = Phase::Handshake;
    }

    /// Drop the per-exchange scratch — the stream cursor, the captured bytes and
    /// the nybble-round latch — without forgetting what the session achieved
    /// (`seen_party`, `traded_away`, the counters). Called whenever a session
    /// ends or is backed out of, so a half-finished exchange or a held nybble
    /// reply can never leak into the next one.
    fn clear_scratch(&mut self) {
        self.stream_pos = 0;
        self.received.clear();
        self.in_nybble_round = false;
        self.nybble_reply = NYBBLE_BASE;
        self.room = Room::Unknown;
        self.last_action = None;
    }

    /// One byte of the clocked exchange with no view of the running game — the
    /// pure-protocol entry point used by the tests.
    #[cfg(test)]
    pub fn feed_blind(&mut self, sent: u8) -> u8 {
        self.feed(sent, &mut NoHost)
    }

    /// One byte of the clocked exchange: the game sent `sent`, we shift back the
    /// return value.
    ///
    /// `host` is how the partner finds out **which room it is in**. Nothing on
    /// the wire says: `CableClub_DoBattleOrTrade` runs the identical byte
    /// sequence for a trade and for a battle, so a partner that guesses answers
    /// battle turns with trade answers — which is what crashed the game. See
    /// [`super::battle`].
    pub fn feed(&mut self, sent: u8, host: &mut dyn Host) -> u8 {
        if self.phase == Phase::Exchanging {
            // Deliberately *before* the cancel check below: inside a block, $6F
            // is ordinary party data (a stat, a name character, a patch offset)
            // and must be answered from our stream, not treated as a cancel.
            return self.feed_exchanging(sent, host);
        }

        // "Let me out." Handled here rather than inside one phase's branch,
        // because the game can send it in any of them — see `feed_cancel`.
        if sent == CANCEL_BYTE {
            return self.feed_cancel();
        }

        // A block preamble anywhere else means the game has started the section
        // exchange. This transfer *is* transfer 0 of it, so answer with the
        // first byte of our stream rather than swallowing it.
        if sent == PREAMBLE_BYTE {
            self.phase = Phase::Exchanging;
            self.stream_pos = 0;
            self.received.clear();
            return self.feed_exchanging(sent, host);
        }

        // (Re-)handshake. Safe to accept at any non-exchanging moment: the game
        // only ever puts $01 on the wire while establishing, and it re-enters
        // that loop after CloseLinkConnection.
        if sent == ESTABLISH_INTERNAL {
            self.phase = Phase::Connected;
            self.in_nybble_round = false;
            return ESTABLISH_EXTERNAL;
        }

        // Zero bytes are the padding that separates nybble rounds. Mirror them,
        // and use them as the round separator.
        if sent == 0x00 {
            self.in_nybble_round = false;
            return 0x00;
        }

        if sent & 0xF0 == NYBBLE_BASE {
            return self.feed_nybble(sent & 0x0F, host);
        }

        if sent & 0xF0 == MENU_BASE {
            // Agree with whatever the player chose in the link menu. A partner
            // that always agrees is exactly what a cooperative second player
            // does, and it means "TRADE CENTER" is entered on the player's
            // button press rather than on ours.
            //
            // Note the room, because it is the only moment it crosses the
            // cable. A selection only counts once a button is pressed (bits
            // 2-3); before that the byte just follows the moving cursor.
            if sent & 0x0C != 0 {
                self.room = match sent & 0x03 {
                    0 => Room::TradeCenter,
                    1 => Room::Colosseum,
                    _ => Room::Cancelled,
                };
            }
            return sent;
        }

        // Anything else: the protocol's "nothing to send".
        party::NO_DATA_BYTE
    }

    /// Answer `$6F`, the "back out of the trade" nybble — the byte that lets
    /// the player leave the Cable Club trade screen.
    ///
    /// `pokered`'s `.cancelMenuItem_APressed`
    /// ([`engine/link/cable_club.asm`](https://github.com/pret/pokered/tree/master/engine/link/cable_club.asm))
    /// writes `$F` to `wSerialExchangeNybbleSendData`, runs one
    /// `Serial_SyncAndExchangeNybble` round, and **only** falls through to
    /// `ReturnToCableClubRoom` when the round comes back `$F`
    /// (`cp $f / jr nz, .cancelMenuItem_Loop`). Anything else and the player is
    /// left on the trade screen pressing A for ever, with `wLinkState` pinned at
    /// `LINK_STATE_TRADING` — measured at 60 presses in
    /// `examples/gen1_cancel_exit.rs` and at 2,475 in the original report.
    ///
    /// Answering it is the only way *off the trade screen*. It is **not** the
    /// way out of the Cable Club: `ReturnToCableClubRoom` reloads the same map,
    /// and the Trade Center has no warp events at all
    /// (`data/maps/objects/TradeCenter.asm`; `wNumberOfWarps` reads 0), so the
    /// player is back in a room with no door and has to press START -> RESET.
    /// That is the cartridge, not us — see
    /// [`docs/gen1/leaving-the-cable-club.md`](../../../docs/gen1/leaving-the-cable-club.md).
    ///
    /// Three things make this a whole-state-machine concern rather than one
    /// branch of the selection round:
    ///
    /// * `wSerialExchangeNybbleSendData` is a **latch**. Once the player has
    ///   cancelled, every later `Serial_SyncAndExchangeNybble` keeps sending
    ///   `$6F` — through `CableClub_DoBattleOrTradeAgain`'s rendezvous sync, for
    ///   instance — until `TradeCenter_SelectMon` resets it to `1`.
    /// * A round is 11–12 transfers of the same byte (`.loop1` plus ten
    ///   `.loop2` iterations), and the game keeps the **last** one it saw, so
    ///   every byte of the run has to be answered the same way.
    /// * If a round is ever answered wrongly the game simply asks again, from
    ///   whatever phase we have since moved into. Answering `$6F` only in
    ///   [`Phase::SelectMon`] therefore makes the *second* attempt unanswerable,
    ///   which is a trap with no way out.
    ///
    /// Afterwards the link is still up — the game returns to the Cable Club room
    /// with `LINK_STATE_IN_CABLE_CLUB` and can walk back onto the console for
    /// another session — so the partner lands in [`Phase::Connected`], with the
    /// exchange scratch cleared so nothing from the abandoned trade carries over.
    fn feed_cancel(&mut self) -> u8 {
        self.clear_scratch();
        self.phase = Phase::Connected;
        CANCEL_BYTE
    }

    fn feed_nybble(&mut self, nybble: u8, host: &mut dyn Host) -> u8 {
        if !self.in_nybble_round {
            self.in_nybble_round = true;
            let (reply, next) = self.decide_round(nybble, host);
            self.nybble_reply = reply;
            self.phase = next;
        }
        self.nybble_reply
    }

    /// One step of a 32-bit xorshift. Not cryptography — the requirement is
    /// only that the opponent's choice is not a function of the player's own
    /// button presses, which is what it used to be.
    fn roll(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    /// Answer one battle turn.
    fn decide_battle(&mut self, host: &mut dyn Host) -> (u8, Phase) {
        // The battle is over as soon as the game leaves `LINK_STATE_BATTLING`;
        // from there the Cable Club's own rendezvous sync resumes and must be
        // answered with nybble 0, not with a battle action.
        if let Some(state) = host.link_state() {
            if state != battle::LINK_STATE_BATTLING {
                return (NYBBLE_BASE, Phase::Connected);
            }
        }
        let fallback = self
            .party
            .mons
            .first()
            .map(|m| m.moves)
            .unwrap_or([0, 0, 0, 0]);
        let roll = self.roll();
        let action = battle::choose(host, fallback, roll);
        self.last_action = Some(action);
        self.battle_turns += 1;
        (NYBBLE_BASE | action.nybble(), Phase::Battling)
    }

    /// Choose this round's answer and the phase the *next* round will be in.
    ///
    /// [`NYBBLE_CANCEL`] never reaches here — [`Self::feed`] intercepts `$6F`
    /// for [`Self::feed_cancel`] in *every* phase, which is the point: a cancel
    /// answered by only one of these arms is a cancel that stops working as
    /// soon as the phase moves on.
    fn decide_round(&mut self, nybble: u8, host: &mut dyn Host) -> (u8, Phase) {
        debug_assert_ne!(nybble, NYBBLE_CANCEL, "$6F is handled by feed_cancel");
        if self.phase == Phase::Battling {
            return self.decide_battle(host);
        }
        match self.phase {
            Phase::SelectMon => (NYBBLE_BASE | (self.offer_index & 0x0F), Phase::Confirm),
            Phase::Confirm => match nybble {
                NYBBLE_DECLINE => (NYBBLE_BASE | NYBBLE_DECLINE, Phase::SelectMon),
                _ => {
                    // Accept. The player's Pokémon is gone from their party at
                    // this point, so record it as the one they traded away.
                    self.trades_completed += 1;
                    (NYBBLE_BASE | NYBBLE_ACCEPT, Phase::PostTrade)
                }
            },
            Phase::PostTrade => (NYBBLE_BASE | NYBBLE_ACCEPT, Phase::Connected),
            // Handshake / Connected / Exchanging: the rendezvous sync. Nybble 0.
            _ => (NYBBLE_BASE, Phase::Connected),
        }
    }

    fn feed_exchanging(&mut self, sent: u8, host: &mut dyn Host) -> u8 {
        self.received.push(sent);
        let reply = self
            .stream
            .get(self.stream_pos)
            .copied()
            .unwrap_or(party::NO_DATA_BYTE);
        self.stream_pos += 1;
        if self.stream_pos >= self.stream.len() {
            self.finish_exchange(host);
        }
        reply
    }

    fn finish_exchange(&mut self, host: &mut dyn Host) {
        self.exchanges_completed += 1;
        self.seen_party = decode_received_party(&self.received);
        // **The one place the two rooms diverge.** Both send the identical 644
        // bytes; what follows is a trade selection in one room and a battle
        // turn in the other. `wLinkState` is `LINK_STATE_START_BATTLE` at this
        // exact moment — `cable_club.asm` promotes it to `LINK_STATE_BATTLING`
        // immediately afterwards — so the game has already told us, if we can
        // read it. Otherwise fall back to the link-menu selection.
        self.phase = match host.link_state() {
            Some(battle::LINK_STATE_START_BATTLE) | Some(battle::LINK_STATE_BATTLING) => {
                Phase::Battling
            }
            Some(_) => Phase::SelectMon,
            None if self.room == Room::Colosseum => Phase::Battling,
            None => Phase::SelectMon,
        };
        self.in_nybble_round = false;
        self.stream_pos = 0;
    }

    /// Record which Pokémon the player gave up, called once the trade is known
    /// to have gone through. Kept separate from [`Self::feed`] because the index
    /// only becomes meaningful after the selection round.
    pub fn note_traded_away(&mut self, index: u8) {
        if let Some(p) = self.seen_party.as_ref() {
            self.traded_away = p.mons.get(index as usize).cloned();
        }
    }
}

/// The random-number list we clock back. The game ignores ours when it is the
/// clock master (it uses its own), but they must still be well formed —
/// strictly below the preamble byte, and never zero or the no-data byte,
/// because the receiver's "find the start of the list" loop skips exactly
/// those.
fn make_random() -> [u8; RANDOM_BLOCK_LEN] {
    let mut random = [0u8; RANDOM_BLOCK_LEN];
    for (i, r) in random.iter_mut().enumerate() {
        *r = 1 + (i as u8).wrapping_mul(23) % 0xFB;
    }
    random
}

/// Concatenate the three blocks into the stream we clock back, leading each with
/// one extra preamble byte for the receiver's re-sync read.
fn build_stream(party_block: &[u8], patch_block: &[u8]) -> Vec<u8> {
    let random = make_random();

    let mut s = Vec::with_capacity(SECTION_STREAM_LEN);
    s.extend(std::iter::repeat(PREAMBLE_BYTE).take(1 + RANDOM_PREAMBLE));
    s.extend_from_slice(&random);
    s.extend(std::iter::repeat(PREAMBLE_BYTE).take(1 + PARTY_PREAMBLE));
    s.extend_from_slice(party_block);
    s.extend(std::iter::repeat(PREAMBLE_BYTE).take(1 + PATCH_PREAMBLE));
    s.extend_from_slice(patch_block);
    s
}

/// Split a received stream into its three blocks the way the *receiver* does.
///
/// The old version indexed at fixed offsets, and it was wrong against a real
/// cartridge — `seen_party()` returned `None` after every genuine exchange, in
/// both Cable Club rooms, and had never once worked. The reason is in
/// `Serial_ExchangeBytes` (`home/serial.asm`): while `hSerialIgnoringInitialData`
/// is set the sender **re-sends its block's first byte** and stores nothing,
/// and it only starts storing once it has *received* a preamble byte. So how
/// many leading copies arrive depends on when the other console's preamble
/// turns up, and no fixed offset can be right. (The unit tests missed it
/// because they fed the partner our own framing back.)
///
/// The fix is to find each block the way the hardware does: skip to the run of
/// `$FD`, skip the whole run however long it is, then take exactly the block's
/// length. Safe because none of the three block bodies can begin with `$FD` —
/// the random list is generated strictly below it, the party block begins with
/// a trainer name in the game's own character encoding, and the patch list
/// holds 1-based offsets no greater than 252.
fn split_blocks(rx: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let mut i = 0usize;
    // Skip to the next run of `$FD` and past all of it; the data starts after.
    let skip_preamble = |i: &mut usize| -> Option<()> {
        while *rx.get(*i)? != PREAMBLE_BYTE {
            *i += 1;
        }
        while *rx.get(*i)? == PREAMBLE_BYTE {
            *i += 1;
        }
        Some(())
    };

    skip_preamble(&mut i)?;
    let random = rx.get(i..i + RANDOM_BLOCK_LEN)?;
    i += RANDOM_BLOCK_LEN;

    skip_preamble(&mut i)?;
    let party = rx.get(i..i + PARTY_BLOCK_LEN)?;
    i += PARTY_BLOCK_LEN;

    // The patch list is allowed to arrive short, and against a real cartridge
    // it always does — by exactly one byte. Our own stream leads each block
    // with one re-sync copy; the game re-sends its first byte until it sees our
    // preamble and then once more, so its stream is one transfer longer than
    // ours and we stop clocking before its final byte. That byte is the patch
    // list's trailing padding — the list is self-terminating with `$FF`, so
    // nothing is lost. Requiring the full 197 here is a decode failure on every
    // genuine exchange.
    skip_preamble(&mut i)?;
    let patch = rx.get(i..(i + PATCH_BLOCK_LEN).min(rx.len()))?;
    Some((random, party, patch))
}

/// Decode the party the game sent us out of the captured stream.
fn decode_received_party(rx: &[u8]) -> Option<Party> {
    let (_random, party_block, patch) = split_blocks(rx)?;
    let mut data = party_block.to_vec();
    party::unpatch(&mut data, patch);

    let trainer_name = text::decode(&data[..NAME_LENGTH]);
    let count = *data.get(party::OFF_PARTY_COUNT)? as usize;
    if count == 0 || count > party::PARTY_LENGTH {
        return None;
    }
    let mut mons = Vec::with_capacity(count);
    for i in 0..count {
        let s = party::OFF_MONS + i * party::MON_STRUCT_LENGTH;
        let ot = party::OFF_OT_NAMES + i * NAME_LENGTH;
        let nick = party::OFF_NICKNAMES + i * NAME_LENGTH;
        let m = Mon::from_bytes(
            data.get(s..s + party::MON_STRUCT_LENGTH)?,
            text::decode(data.get(ot..ot + NAME_LENGTH)?),
            text::decode(data.get(nick..nick + NAME_LENGTH)?),
        )?;
        mons.push(m);
    }
    Some(Party { trainer_name, mons })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::romdata::Stats;

    fn test_mon(species: u8) -> Mon {
        Mon {
            species,
            current_hp: 50,
            status: 0,
            type1: 0,
            type2: 0,
            catch_rate: 45,
            moves: [1, 2, 0, 0],
            pp: [35, 25, 0, 0],
            ot_id: 0x1234,
            experience: 125_000,
            stat_exp: Stats::default(),
            dv_attack: 15,
            dv_defense: 15,
            dv_speed: 15,
            dv_special: 15,
            level: 50,
            stats: Stats {
                hp: 150,
                attack: 100,
                defense: 90,
                speed: 110,
                special: 120,
            },
            ot_name: "AI".into(),
            nickname: "GIFT".into(),
        }
    }

    fn partner() -> LinkPartner {
        LinkPartner::new(Party {
            trainer_name: "AI".into(),
            mons: vec![test_mon(21)],
        })
        .unwrap()
    }

    #[test]
    fn the_stream_is_exactly_644_bytes() {
        // 18 + 425 + 201. If this number moves, every offset below moves with
        // it and the game receives a shifted party.
        assert_eq!(SECTION_STREAM_LEN, 644);
        assert_eq!(partner().stream.len(), 644);
    }

    #[test]
    fn the_handshake_makes_the_game_the_clock_master() {
        let mut p = partner();
        assert_eq!(p.phase(), Phase::Handshake);
        // The game offers to take the clock with $01; answering $02 is what
        // sets its hSerialConnectionStatus to USING_INTERNAL_CLOCK.
        assert_eq!(p.feed_blind(ESTABLISH_INTERNAL), ESTABLISH_EXTERNAL);
        assert_eq!(p.phase(), Phase::Connected);
    }

    #[test]
    fn the_rendezvous_sync_answers_nybble_zero() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        assert_eq!(p.feed_blind(0x60), 0x60);
        assert_eq!(p.feed_blind(0x60), 0x60);
        assert_eq!(p.feed_blind(0x00), 0x00);
    }

    #[test]
    fn the_link_menu_agrees_with_the_player() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        // $D4 = A pressed on TRADE CENTER (item 0, button A -> +4).
        assert_eq!(p.feed_blind(0xD4), 0xD4);
        // ...and cancel is agreed with too, rather than forced through.
        assert_eq!(p.feed_blind(0xDA), 0xDA);
    }

    /// Drive a whole section exchange and check both directions.
    fn run_exchange(p: &mut LinkPartner, their_stream: &[u8]) -> Vec<u8> {
        their_stream.iter().map(|&b| p.feed_blind(b)).collect()
    }

    #[test]
    fn a_full_exchange_delivers_our_party_where_the_game_will_look_for_it() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);

        // Model the receiver: it re-sends its first byte until it sees a
        // preamble, then stores the next N bytes. Build the game's side of the
        // wire the same way our stream is built, then check what it "stores".
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99), test_mon(0x54)],
        };
        let (their_block, their_patch) = other.to_blocks().unwrap();
        let their_stream = build_stream(&their_block, &their_patch);

        let ours = run_exchange(&mut p, &their_stream);
        assert_eq!(ours.len(), SECTION_STREAM_LEN);
        assert_eq!(p.phase(), Phase::SelectMon);
        assert_eq!(p.exchanges_completed(), 1);

        // Find the party block in our own outgoing stream the way the receiver
        // finds it, rather than at an offset we chose — the fixed offsets are
        // exactly what was wrong on the receive side.
        let (_r, data, _p) = split_blocks(&ours).expect("our stream is well formed");
        assert_eq!(text::decode(&data[..NAME_LENGTH]), "AI");
        assert_eq!(data[party::OFF_PARTY_COUNT], 1);
        assert_eq!(data[party::OFF_MONS], 21);
    }

    #[test]
    fn we_decode_the_party_the_game_offered_us() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99), test_mon(0x54)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));

        let seen = p.seen_party().expect("party decoded");
        assert_eq!(seen.trainer_name, "ASH");
        assert_eq!(seen.mons.len(), 2);
        assert_eq!(seen.mons[0].species, 0x99);
        assert_eq!(seen.mons[1].species, 0x54);
        assert_eq!(seen.mons[0].nickname, "GIFT");
        assert_eq!(seen.mons[0].ot_name, "AI");
        assert_eq!(seen.mons[0].level, 50);
    }

    #[test]
    fn selection_then_confirmation_completes_a_trade() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));
        assert_eq!(p.phase(), Phase::SelectMon);

        // Player picks their slot 0; we answer with our own offered slot.
        assert_eq!(p.feed_blind(0x60), 0x60);
        assert_eq!(p.feed_blind(0x60), 0x60);
        p.feed_blind(0x00); // round separator
        assert_eq!(p.phase(), Phase::Confirm);

        // Player confirms (nybble 2); so do we.
        assert_eq!(p.feed_blind(0x62), 0x62);
        p.feed_blind(0x00);
        assert_eq!(p.trades_completed(), 1);
        assert_eq!(p.phase(), Phase::PostTrade);

        // Post-trade sync, then back to waiting for the next exchange.
        p.feed_blind(0x62);
        p.feed_blind(0x00);
        assert_eq!(p.phase(), Phase::Connected);
    }

    #[test]
    fn a_cancelled_confirmation_returns_to_selection_without_trading() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));
        p.feed_blind(0x60);
        p.feed_blind(0x00);
        // Nybble 1 = "no trade".
        assert_eq!(p.feed_blind(0x61), 0x61);
        p.feed_blind(0x00);
        assert_eq!(p.trades_completed(), 0);
        assert_eq!(p.phase(), Phase::SelectMon);
    }

    #[test]
    fn cancelling_the_selection_screen_is_agreed_with() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));
        assert_eq!(p.feed_blind(0x6F), 0x6F);
        p.feed_blind(0x00);
        assert_eq!(p.phase(), Phase::Connected);
        assert_eq!(p.trades_completed(), 0);
    }

    // --- leaving the trade room -------------------------------------------
    //
    // `$6F` is the byte that lets the player out of the Cable Club, and the
    // game will send it from more than one phase (see `feed_cancel`). The
    // original test above only ever exercised it from `SelectMon`, which is why
    // the other phases were broken and invisible. These drive it from *every*
    // phase the machine can be in, and assert the phase we land in as well as
    // the byte — a test that checks only the byte would miss the same class of
    // bug again, because the failure is "the second attempt is unanswerable".

    /// The other party on the wire, and the stream the game would clock at us.
    fn their_stream() -> Vec<u8> {
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99), test_mon(0x54)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        build_stream(&b, &patch)
    }

    /// Drive a fresh partner into `phase` using only real wire bytes, so the
    /// state it ends up in is one a running game can actually produce.
    fn partner_in(phase: Phase) -> LinkPartner {
        let mut p = partner();
        if phase == Phase::Handshake {
            return p;
        }
        p.feed_blind(ESTABLISH_INTERNAL);
        match phase {
            Phase::Connected => {}
            Phase::Exchanging => {
                p.feed_blind(PREAMBLE_BYTE);
            }
            Phase::SelectMon => {
                run_exchange(&mut p, &their_stream());
            }
            Phase::Confirm => {
                run_exchange(&mut p, &their_stream());
                p.feed_blind(0x60); // the player picked party slot 0
                p.feed_blind(0x00);
            }
            Phase::PostTrade => {
                run_exchange(&mut p, &their_stream());
                p.feed_blind(0x60);
                p.feed_blind(0x00);
                p.feed_blind(0x62); // ...and confirmed it
                p.feed_blind(0x00);
            }
            Phase::Battling => {
                p.feed_blind(0xD5); // A on COLOSSEUM, so the room is known
                run_exchange(&mut p, &their_stream());
            }
            Phase::Handshake => unreachable!(),
        }
        assert_eq!(p.phase(), phase, "helper did not reach {phase:?}");
        p
    }

    /// Every phase the game can send `$6F` from. `Exchanging` is deliberately
    /// absent: in the middle of a block that byte is party data, not a cancel,
    /// and it has its own test below.
    const CANCELLABLE: [Phase; 5] = [
        Phase::Handshake,
        Phase::Connected,
        Phase::SelectMon,
        Phase::Confirm,
        Phase::PostTrade,
    ];

    /// One `Serial_SyncAndExchangeNybble` round as the game runs it: the value
    /// goes out once in `.loop1` and ten more times in `.loop2`, then ten zero
    /// bytes in `.loop3`. The game keeps the **last** `$6x` it saw, so every
    /// byte of the run has to come back the same.
    fn nybble_round(p: &mut LinkPartner, byte: u8) -> Vec<u8> {
        let mut out: Vec<u8> = (0..12).map(|_| p.feed_blind(byte)).collect();
        out.extend((0..10).map(|_| p.feed_blind(0x00)));
        out
    }

    #[test]
    fn the_cancel_byte_is_echoed_from_every_phase() {
        for phase in CANCELLABLE {
            let mut p = partner_in(phase);
            assert_eq!(
                p.feed_blind(CANCEL_BYTE),
                CANCEL_BYTE,
                "$6F sent from {phase:?} was not echoed — the game would sit on \
                 the trade screen for ever"
            );
            assert_eq!(
                p.phase(),
                Phase::Connected,
                "$6F sent from {phase:?} left the partner in the wrong phase"
            );
        }
    }

    #[test]
    fn a_whole_cancel_round_is_echoed_from_every_phase() {
        for phase in CANCELLABLE {
            let mut p = partner_in(phase);
            let replies = nybble_round(&mut p, CANCEL_BYTE);
            assert!(
                replies[..12].iter().all(|&b| b == CANCEL_BYTE),
                "from {phase:?} the round answered {replies:02X?}"
            );
            assert!(replies[12..].iter().all(|&b| b == 0x00));
            assert_eq!(p.phase(), Phase::Connected);
        }
    }

    /// The bug this class of test exists for: the game re-sends the cancel from
    /// whatever phase the partner has since moved into, so a cancel has to keep
    /// working when it is repeated.
    #[test]
    fn a_repeated_cancel_keeps_being_answered() {
        let mut p = partner_in(Phase::SelectMon);
        for attempt in 1..=4 {
            let replies = nybble_round(&mut p, CANCEL_BYTE);
            assert!(
                replies[..12].iter().all(|&b| b == CANCEL_BYTE),
                "cancel attempt {attempt} answered {replies:02X?}"
            );
            assert_eq!(p.phase(), Phase::Connected);
        }
        assert_eq!(p.trades_completed(), 0);
    }

    /// A cancel is not a trade, whatever phase it interrupts. Before the fix a
    /// `$6F` arriving in `Confirm` fell into the "accept" arm and bumped this
    /// counter while answering `$62`.
    #[test]
    fn a_cancel_never_counts_as_a_trade() {
        for phase in CANCELLABLE {
            let mut p = partner_in(phase);
            let before = p.trades_completed();
            nybble_round(&mut p, CANCEL_BYTE);
            assert_eq!(
                p.trades_completed(),
                before,
                "a cancel from {phase:?} was recorded as a trade"
            );
        }
    }

    /// Inside a section exchange `$6F` is a data byte — a stat, a name
    /// character, a patch-list offset — and must be answered from our stream.
    #[test]
    fn the_cancel_byte_is_data_inside_an_exchange() {
        let mut p = partner_in(Phase::Exchanging);
        let expected = p.stream[p.stream_pos];
        assert_eq!(p.feed_blind(CANCEL_BYTE), expected);
        assert_eq!(p.phase(), Phase::Exchanging);
        assert_eq!(p.received.last(), Some(&CANCEL_BYTE));
    }

    /// Leaving has to be repeatable: after backing out, the game walks the
    /// player back onto the console and `CableClub_DoBattleOrTradeAgain` runs
    /// the whole three-block exchange again.
    #[test]
    fn a_new_session_works_after_a_cancel_from_any_phase() {
        for phase in CANCELLABLE {
            let mut p = partner_in(phase);
            nybble_round(&mut p, CANCEL_BYTE);
            // `wSerialExchangeNybbleSendData` is still latched at $F, so the
            // rendezvous sync that opens the next session sends $6F too.
            nybble_round(&mut p, CANCEL_BYTE);
            let ours = run_exchange(&mut p, &their_stream());
            assert_eq!(ours.len(), SECTION_STREAM_LEN, "from {phase:?}");
            assert_eq!(p.phase(), Phase::SelectMon, "from {phase:?}");
            let seen = p.seen_party().expect("the new party decoded");
            assert_eq!(seen.mons.len(), 2, "stale data leaked in from {phase:?}");
        }
    }

    /// The maintainer's report, end to end at the protocol level: trade, then
    /// leave. The post-trade rounds put the partner in `Connected`, and the
    /// cancel that follows has to be answered from there.
    #[test]
    fn the_player_can_leave_after_a_completed_trade() {
        let mut p = partner_in(Phase::PostTrade);
        // `.tradeCompleted`'s sync, then `CableClub_DoBattleOrTradeAgain`'s.
        nybble_round(&mut p, 0x62);
        assert_eq!(p.phase(), Phase::Connected);
        nybble_round(&mut p, 0x60);
        // The game runs one more full exchange before showing the selection
        // screen again — but if anything cut that short, the cancel below is
        // still sent from `Connected`, and that is the case that was broken.
        assert_eq!(p.trades_completed(), 1);
        let replies = nybble_round(&mut p, CANCEL_BYTE);
        assert!(replies[..12].iter().all(|&b| b == CANCEL_BYTE));
        assert_eq!(p.phase(), Phase::Connected);
        assert_eq!(p.trades_completed(), 1);
    }

    #[test]
    fn a_second_exchange_starts_cleanly_after_a_trade() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        let stream = build_stream(&b, &patch);
        run_exchange(&mut p, &stream);
        p.feed_blind(0x60);
        p.feed_blind(0x00);
        p.feed_blind(0x62);
        p.feed_blind(0x00);
        p.feed_blind(0x62);
        p.feed_blind(0x00);
        // Straight back into another full exchange.
        let ours = run_exchange(&mut p, &stream);
        assert_eq!(ours.len(), SECTION_STREAM_LEN);
        assert_eq!(p.exchanges_completed(), 2);
        assert_eq!(p.phase(), Phase::SelectMon);
    }

    /// A host that says what room it is in, so the Colosseum path can be driven
    /// without booting a ROM.
    struct FakeHost {
        state: u8,
        active: Option<super::battle::ActiveMon>,
    }
    impl Host for FakeHost {
        fn link_state(&mut self) -> Option<u8> {
            Some(self.state)
        }
        fn active(&mut self) -> Option<super::battle::ActiveMon> {
            self.active
        }
    }

    fn colosseum_host() -> FakeHost {
        FakeHost {
            state: super::battle::LINK_STATE_START_BATTLE,
            active: Some(super::battle::ActiveMon {
                species: 21,
                hp: 40,
                party_pos: 0,
                moves: [1, 2, 0, 0],
                pp: [10, 10, 0, 0],
            }),
        }
    }

    fn other_party() -> Party {
        Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99), test_mon(0x54)],
        }
    }

    /// **The bug.** A Colosseum session runs the identical 644-byte exchange a
    /// trade does and then asks a question with the identical encoding — and
    /// the trade state machine used to answer it, sending "confirm" (nybble 2)
    /// as move slot 2. A Pokémon with two moves has nothing in slot 2, and the
    /// game jumps out of its own program.
    #[test]
    fn a_colosseum_round_is_answered_as_a_battle_action_not_a_trade_phase() {
        let mut p = partner();
        let mut host = colosseum_host();
        p.feed(ESTABLISH_INTERNAL, &mut host);
        p.feed(0xD5, &mut host); // A on COLOSSEUM
        let (b, patch) = other_party().to_blocks().unwrap();
        for byte in build_stream(&b, &patch) {
            p.feed(byte, &mut host);
        }
        assert_eq!(p.phase(), Phase::Battling);
        assert_eq!(p.room(), Room::Colosseum);

        host.state = super::battle::LINK_STATE_BATTLING;
        // Twenty turns, each answered as an action the receiver bounds-checks.
        for _ in 0..20 {
            let reply = p.feed(0x60, &mut host);
            assert_eq!(reply & 0xF0, NYBBLE_BASE);
            let nybble = reply & 0x0F;
            // Only ever a populated move slot, or one of the three defined
            // special actions. The partner's Pokémon has moves in slots 0 and 1.
            assert!(
                nybble < 2 || (0x0D..=0x0F).contains(&nybble),
                "answered {nybble:#x}, which is neither a populated move slot nor a \
                 defined action"
            );
            p.feed(0x00, &mut host);
        }
        assert_eq!(p.battle_turns(), 20);
        // ...and it is a battle, not a trade. This used to count one per fight.
        assert_eq!(p.trades_completed(), 0);
    }

    /// The trade path is untouched by all of that.
    #[test]
    fn a_trade_session_still_reaches_the_selection_round() {
        let mut p = partner();
        let mut host = FakeHost {
            state: super::battle::LINK_STATE_START_TRADE,
            active: None,
        };
        p.feed(ESTABLISH_INTERNAL, &mut host);
        p.feed(0xD4, &mut host); // A on TRADE CENTER
        let (b, patch) = other_party().to_blocks().unwrap();
        for byte in build_stream(&b, &patch) {
            p.feed(byte, &mut host);
        }
        assert_eq!(p.room(), Room::TradeCenter);
        assert_eq!(p.phase(), Phase::SelectMon);
    }

    /// The game is the authority on which room it is in, not the menu byte we
    /// echoed: `LinkMenu` resolves a disagreement in favour of whoever drives
    /// the clock, which is always the game.
    #[test]
    fn the_games_link_state_beats_the_link_menu() {
        let mut p = partner();
        let mut host = FakeHost {
            state: super::battle::LINK_STATE_START_TRADE,
            active: None,
        };
        p.feed(ESTABLISH_INTERNAL, &mut host);
        p.feed(0xD5, &mut host); // the menu said COLOSSEUM...
        let (b, patch) = other_party().to_blocks().unwrap();
        for byte in build_stream(&b, &patch) {
            p.feed(byte, &mut host);
        }
        // ...but the game says it is trading, so it is trading.
        assert_eq!(p.room(), Room::Colosseum);
        assert_eq!(p.phase(), Phase::SelectMon);
    }

    /// With no view of the machine at all, the link-menu byte is all there is —
    /// and it is enough to stop a trade answer reaching a battle turn.
    #[test]
    fn the_link_menu_alone_still_identifies_the_colosseum() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        p.feed_blind(0xD5);
        let (b, patch) = other_party().to_blocks().unwrap();
        for byte in build_stream(&b, &patch) {
            p.feed_blind(byte);
        }
        assert_eq!(p.phase(), Phase::Battling);
    }

    /// **The receive-side framing.** The old decoder indexed at fixed offsets
    /// and was wrong against every real cartridge; the test that was supposed to
    /// catch it fed the partner *our own* framing back. So this one builds the
    /// stream the way `Serial_ExchangeBytes` (`home/serial.asm`) does: the
    /// sender re-sends its block's first byte until it receives a preamble and
    /// then once more, so the number of leading copies is not ours to choose —
    /// and its stream ends up one transfer longer than ours, so we never clock
    /// its final byte.
    #[test]
    fn a_party_is_decoded_from_the_framing_a_cartridge_actually_sends() {
        // At least one: `Serial_ExchangeBytes` re-sends its first byte until it
        // *receives* a preamble and then once more, so a real stream never
        // carries fewer than one extra copy — and may carry several, which is
        // exactly why no fixed offset works.
        for extra_resync in 1..5usize {
            let mut p = partner();
            let (block, patch) = other_party().to_blocks().unwrap();
            let mut wire = Vec::new();
            for (preamble, body) in [
                (RANDOM_PREAMBLE, &make_random()[..]),
                (PARTY_PREAMBLE, &block[..]),
                (PATCH_PREAMBLE, &patch[..]),
            ] {
                wire.extend(
                    std::iter::repeat(PREAMBLE_BYTE).take(preamble + extra_resync),
                );
                wire.extend_from_slice(body);
            }
            // We stop clocking after our own stream length, whatever the game
            // still had to send.
            wire.truncate(SECTION_STREAM_LEN.min(wire.len()));

            p.feed_blind(ESTABLISH_INTERNAL);
            for b in &wire {
                p.feed_blind(*b);
            }
            let seen = p.seen_party().unwrap_or_else(|| {
                panic!("decode failed with {extra_resync} extra re-sync bytes")
            });
            assert_eq!(seen.trainer_name, "ASH");
            assert_eq!(seen.mons.len(), 2);
            assert_eq!(seen.mons[0].species, 0x99);
            assert_eq!(seen.mons[1].species, 0x54);
        }
    }

    #[test]
    fn reset_returns_to_waiting_for_a_handshake() {
        let mut p = partner();
        p.feed_blind(ESTABLISH_INTERNAL);
        p.feed_blind(PREAMBLE_BYTE);
        assert_eq!(p.phase(), Phase::Exchanging);
        p.reset();
        assert_eq!(p.phase(), Phase::Handshake);
        assert_eq!(p.feed_blind(ESTABLISH_INTERNAL), ESTABLISH_EXTERNAL);
    }
}

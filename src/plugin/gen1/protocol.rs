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

/// Where in the received stream each block's data lands. Derived in the module
/// docs; pinned by a unit test so an off-by-one cannot creep back.
const RX_PARTY: usize = 1 + (1 + RANDOM_PREAMBLE + RANDOM_BLOCK_LEN);
const RX_PATCH: usize = RX_PARTY + 1 + (PARTY_PREAMBLE + PARTY_BLOCK_LEN);

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
        }
    }
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
}

impl LinkPartner {
    pub fn new(party: Party) -> Result<LinkPartner, party::PartyError> {
        let (block, patch) = party.to_blocks()?;
        let stream = build_stream(&block, &patch);
        debug_assert_eq!(stream.len(), SECTION_STREAM_LEN);
        Ok(LinkPartner {
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
    }

    /// One byte of the clocked exchange: the game sent `sent`, we shift back the
    /// return value.
    pub fn feed(&mut self, sent: u8) -> u8 {
        if self.phase == Phase::Exchanging {
            // Deliberately *before* the cancel check below: inside a block, $6F
            // is ordinary party data (a stat, a name character, a patch offset)
            // and must be answered from our stream, not treated as a cancel.
            return self.feed_exchanging(sent);
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
            return self.feed_exchanging(sent);
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
            return self.feed_nybble(sent & 0x0F);
        }

        if sent & 0xF0 == MENU_BASE {
            // Agree with whatever the player chose in the link menu. A partner
            // that always agrees is exactly what a cooperative second player
            // does, and it means "TRADE CENTER" is entered on the player's
            // button press rather than on ours.
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
    /// left on the trade screen pressing A for ever — and since the Trade Center
    /// has no warp events at all (`data/maps/objects/TradeCenter.asm`), that
    /// screen is the *only* exit the room has.
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

    fn feed_nybble(&mut self, nybble: u8) -> u8 {
        if !self.in_nybble_round {
            self.in_nybble_round = true;
            let (reply, next) = self.decide_round(nybble);
            self.nybble_reply = reply;
            self.phase = next;
        }
        self.nybble_reply
    }

    /// Choose this round's answer and the phase the *next* round will be in.
    ///
    /// [`NYBBLE_CANCEL`] never reaches here — [`Self::feed`] intercepts `$6F`
    /// for [`Self::feed_cancel`] in *every* phase, which is the point: a cancel
    /// answered by only one of these arms is a cancel that stops working as
    /// soon as the phase moves on.
    fn decide_round(&mut self, nybble: u8) -> (u8, Phase) {
        debug_assert_ne!(nybble, NYBBLE_CANCEL, "$6F is handled by feed_cancel");
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

    fn feed_exchanging(&mut self, sent: u8) -> u8 {
        self.received.push(sent);
        let reply = self
            .stream
            .get(self.stream_pos)
            .copied()
            .unwrap_or(party::NO_DATA_BYTE);
        self.stream_pos += 1;
        if self.stream_pos >= self.stream.len() {
            self.finish_exchange();
        }
        reply
    }

    fn finish_exchange(&mut self) {
        self.exchanges_completed += 1;
        self.seen_party = decode_received_party(&self.received);
        self.phase = Phase::SelectMon;
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

/// Concatenate the three blocks into the stream we clock back, leading each with
/// one extra preamble byte for the receiver's re-sync read.
fn build_stream(party_block: &[u8], patch_block: &[u8]) -> Vec<u8> {
    // Random numbers: the game ignores ours when it is the clock master (it uses
    // its own list), but they must still be well formed — strictly below the
    // preamble byte, and never zero or the no-data byte, because the receiver's
    // "find the start of the list" loop skips exactly those.
    let mut random = [0u8; RANDOM_BLOCK_LEN];
    for (i, r) in random.iter_mut().enumerate() {
        *r = 1 + (i as u8).wrapping_mul(23) % 0xFB;
    }

    let mut s = Vec::with_capacity(SECTION_STREAM_LEN);
    s.extend(std::iter::repeat(PREAMBLE_BYTE).take(1 + RANDOM_PREAMBLE));
    s.extend_from_slice(&random);
    s.extend(std::iter::repeat(PREAMBLE_BYTE).take(1 + PARTY_PREAMBLE));
    s.extend_from_slice(party_block);
    s.extend(std::iter::repeat(PREAMBLE_BYTE).take(1 + PATCH_PREAMBLE));
    s.extend_from_slice(patch_block);
    s
}

/// Decode the party the game sent us out of the captured stream.
fn decode_received_party(rx: &[u8]) -> Option<Party> {
    if rx.len() < SECTION_STREAM_LEN {
        return None;
    }
    let party_total = PARTY_PREAMBLE + PARTY_BLOCK_LEN;
    let patch_total = PATCH_PREAMBLE + PATCH_BLOCK_LEN;
    let mut block = rx.get(RX_PARTY..RX_PARTY + party_total)?.to_vec();
    let patch = rx.get(RX_PATCH..RX_PATCH + patch_total)?.to_vec();
    // Drop each block's own preamble to get at the data.
    let data = block.split_off(PARTY_PREAMBLE);
    let mut data = data;
    party::unpatch(&mut data, &patch[PATCH_PREAMBLE..]);

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
    use crate::plugin::gen1::romdata::Stats;

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
        assert_eq!(p.feed(ESTABLISH_INTERNAL), ESTABLISH_EXTERNAL);
        assert_eq!(p.phase(), Phase::Connected);
    }

    #[test]
    fn the_rendezvous_sync_answers_nybble_zero() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);
        assert_eq!(p.feed(0x60), 0x60);
        assert_eq!(p.feed(0x60), 0x60);
        assert_eq!(p.feed(0x00), 0x00);
    }

    #[test]
    fn the_link_menu_agrees_with_the_player() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);
        // $D4 = A pressed on TRADE CENTER (item 0, button A -> +4).
        assert_eq!(p.feed(0xD4), 0xD4);
        // ...and cancel is agreed with too, rather than forced through.
        assert_eq!(p.feed(0xDA), 0xDA);
    }

    /// Drive a whole section exchange and check both directions.
    fn run_exchange(p: &mut LinkPartner, their_stream: &[u8]) -> Vec<u8> {
        their_stream.iter().map(|&b| p.feed(b)).collect()
    }

    #[test]
    fn a_full_exchange_delivers_our_party_where_the_game_will_look_for_it() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);

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

        // What the game stores for each block is our stream with each block's
        // leading re-sync byte dropped.
        let stored_party = &ours[RX_PARTY..RX_PARTY + PARTY_PREAMBLE + PARTY_BLOCK_LEN];
        assert!(stored_party[..PARTY_PREAMBLE]
            .iter()
            .all(|&b| b == PREAMBLE_BYTE));
        let data = &stored_party[PARTY_PREAMBLE..];
        assert_eq!(text::decode(&data[..NAME_LENGTH]), "AI");
        assert_eq!(data[party::OFF_PARTY_COUNT], 1);
        assert_eq!(data[party::OFF_MONS], 21);
    }

    #[test]
    fn we_decode_the_party_the_game_offered_us() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);
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
        p.feed(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));
        assert_eq!(p.phase(), Phase::SelectMon);

        // Player picks their slot 0; we answer with our own offered slot.
        assert_eq!(p.feed(0x60), 0x60);
        assert_eq!(p.feed(0x60), 0x60);
        p.feed(0x00); // round separator
        assert_eq!(p.phase(), Phase::Confirm);

        // Player confirms (nybble 2); so do we.
        assert_eq!(p.feed(0x62), 0x62);
        p.feed(0x00);
        assert_eq!(p.trades_completed(), 1);
        assert_eq!(p.phase(), Phase::PostTrade);

        // Post-trade sync, then back to waiting for the next exchange.
        p.feed(0x62);
        p.feed(0x00);
        assert_eq!(p.phase(), Phase::Connected);
    }

    #[test]
    fn a_cancelled_confirmation_returns_to_selection_without_trading() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));
        p.feed(0x60);
        p.feed(0x00);
        // Nybble 1 = "no trade".
        assert_eq!(p.feed(0x61), 0x61);
        p.feed(0x00);
        assert_eq!(p.trades_completed(), 0);
        assert_eq!(p.phase(), Phase::SelectMon);
    }

    #[test]
    fn cancelling_the_selection_screen_is_agreed_with() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        run_exchange(&mut p, &build_stream(&b, &patch));
        assert_eq!(p.feed(0x6F), 0x6F);
        p.feed(0x00);
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
        p.feed(ESTABLISH_INTERNAL);
        match phase {
            Phase::Connected => {}
            Phase::Exchanging => {
                p.feed(PREAMBLE_BYTE);
            }
            Phase::SelectMon => {
                run_exchange(&mut p, &their_stream());
            }
            Phase::Confirm => {
                run_exchange(&mut p, &their_stream());
                p.feed(0x60); // the player picked party slot 0
                p.feed(0x00);
            }
            Phase::PostTrade => {
                run_exchange(&mut p, &their_stream());
                p.feed(0x60);
                p.feed(0x00);
                p.feed(0x62); // ...and confirmed it
                p.feed(0x00);
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
        let mut out: Vec<u8> = (0..12).map(|_| p.feed(byte)).collect();
        out.extend((0..10).map(|_| p.feed(0x00)));
        out
    }

    #[test]
    fn the_cancel_byte_is_echoed_from_every_phase() {
        for phase in CANCELLABLE {
            let mut p = partner_in(phase);
            assert_eq!(
                p.feed(CANCEL_BYTE),
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
        assert_eq!(p.feed(CANCEL_BYTE), expected);
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

    /// The captain's report, end to end at the protocol level: trade, then
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
        p.feed(ESTABLISH_INTERNAL);
        let other = Party {
            trainer_name: "ASH".into(),
            mons: vec![test_mon(0x99)],
        };
        let (b, patch) = other.to_blocks().unwrap();
        let stream = build_stream(&b, &patch);
        run_exchange(&mut p, &stream);
        p.feed(0x60);
        p.feed(0x00);
        p.feed(0x62);
        p.feed(0x00);
        p.feed(0x62);
        p.feed(0x00);
        // Straight back into another full exchange.
        let ours = run_exchange(&mut p, &stream);
        assert_eq!(ours.len(), SECTION_STREAM_LEN);
        assert_eq!(p.exchanges_completed(), 2);
        assert_eq!(p.phase(), Phase::SelectMon);
    }

    #[test]
    fn reset_returns_to_waiting_for_a_handshake() {
        let mut p = partner();
        p.feed(ESTABLISH_INTERNAL);
        p.feed(PREAMBLE_BYTE);
        assert_eq!(p.phase(), Phase::Exchanging);
        p.reset();
        assert_eq!(p.phase(), Phase::Handshake);
        assert_eq!(p.feed(ESTABLISH_INTERNAL), ESTABLISH_EXTERNAL);
    }
}

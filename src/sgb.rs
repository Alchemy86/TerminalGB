//! Super Game Boy — the half of it a **cartridge** can observe.
//!
//! An SGB is a Game Boy on a card inside a SNES cartridge. The Game Boy side is
//! DMG hardware; everything that makes an SGB an SGB (the border, the colour
//! palettes, the SNES sound) lives on the SNES side, behind an ICD2 chip that
//! watches the joypad port. A cartridge talks to that chip by pulsing `P14` and
//! `P15` (`$FF00` bits 4 and 5) — see Pan Docs, ["SGB Command Packet
//! Transfer"](https://gbdev.io/pandocs/SGB_Command_Packet_Transfer.html).
//!
//! **What is modelled here, and why only this.** This module implements the
//! packet *receiver* and exactly one command, `MLT_REQ` (multiplayer joypad
//! request), because those are the only parts a Game Boy program can *read
//! back*. Every other SGB command is one-way: the game sends a border or a
//! palette and the SNES draws it on a screen the cartridge cannot see, so a
//! test ROM cannot check it and a DMG-screen emulator has nothing to show. The
//! joypad register is the one place the SNES answers, and it is what all three
//! published SGB conformance tests measure. Border, palettes and SGB sound are
//! **not** emulated and are a documented non-goal — see `docs/components/sgb.md`.
//!
//! ## The wire protocol, as the tests pin it down
//!
//! A packet is 16 bytes, LSB first, and up to seven of them chain (the low three
//! bits of the first byte are the packet count).
//!
//! * Writing `$00` — **both** select lines low — arms a transfer and throws away
//!   any packet in progress.
//! * A write of `$30` (both high) while armed **starts** the packet. `$00`
//!   without that following `$30` starts nothing: the transfer stays dead and
//!   every later pulse is misread, which is what `sgb-ext-test`'s
//!   `SendPacketShortStart` checks.
//! * Once started, each write of `$30` latches one bit, and **the bit is decided
//!   by the value immediately before it**: `$10` (P15 low) is a `1`, `$20` (P14
//!   low) is a `0`. It is the last line held low that counts, not the first —
//!   `sgb-ext-test` sends `$20 → $10 → $30` and `$10 → $20 → $30` for the same
//!   bit position precisely to tell those two rules apart.
//! * The command runs the moment the last bit of the last packet arrives, **not**
//!   at the stop bit. `sgb-ext-test`'s `SendPacketCorruptStop` terminates a
//!   packet with a `1` bit instead of the usual `0` and still expects the command
//!   to have taken effect.
//!
//! ## `MLT_REQ`, and the "glitched player 3"
//!
//! `MLT_REQ` ($11) takes a two-bit parameter: 0 = one player, 1 = two, 3 = four.
//! `2` is not a legal request and the interesting part of the hardware is what it
//! does anyway: `player_count` becomes **3**, so the joypad index is masked with
//! `3 - 1 = 2` and can only ever be 0 or 2 — the machine alternates between
//! player 1 and player *3*. SameBoy rounds that case up to four players, and that
//! single line is why SameBoy fails `samesuite/sgb/command_mlt_req`.
//!
//! The index itself advances on the **rising edge of P15** (bit 5 of the value
//! written to `$FF00`), masked with `player_count - 1` each time — so a
//! one-player machine never moves and a `$20 → $30` pulse never counts, which is
//! the whole of `command_mlt_req_1_incrementing`.
//!
//! Applying `MLT_REQ` then advances the index `player_count` times more. For 1,
//! 2 and 4 players that is a whole number of turns and cannot be seen; for the
//! illegal count of 3 it leaves the index one step further on than a plain mask
//! would, and `command_mlt_req` measures exactly that. (Physically: the SNES side
//! polls every controller it now believes is attached.)
//!
//! Reading `$FF00` with both select lines high returns `$F - index` in the low
//! nibble when more than one player is configured, and the usual `$F` otherwise.

/// One packet is 16 bytes; a command may chain up to seven of them.
const PACKET_BYTES: usize = 16;
const MAX_PACKETS: usize = 7;
const BUFFER_BYTES: usize = PACKET_BYTES * MAX_PACKETS;
const PACKET_BITS: u16 = (PACKET_BYTES * 8) as u16;

/// `MLT_REQ` — the SGB command id lives in the top five bits of byte 0.
const MLT_REQ: u8 = 0x11;

/// The ICD2 as a cartridge can see it: a packet receiver on `$FF00` and the
/// multiplayer joypad index it maintains.
#[derive(Clone)]
pub struct Sgb {
    /// The select-line bits (`value & 0x30`) of the last write to `$FF00`.
    prev: u8,
    /// Packet bytes received so far.
    buffer: [u8; BUFFER_BYTES],
    /// Bits received into `buffer`.
    bits: u16,
    /// A `$00` was seen; the next `$30` starts a transfer.
    armed: bool,
    /// A transfer is in progress and pulses are being read as bits.
    receiving: bool,
    /// The command has already run; the next pulse just ends the transfer.
    complete: bool,
    /// `player_count - 1`, the mask the joypad index wraps with.
    mask: u8,
    /// Which player the next read reports: 0 = player 1.
    index: u8,
    /// How many players the last `MLT_REQ` asked for (1, 2, 3 or 4).
    players: u8,
}

impl Default for Sgb {
    fn default() -> Self {
        Sgb::new()
    }
}

impl Sgb {
    pub fn new() -> Sgb {
        Sgb {
            // The boot ROM leaves both select lines pulled low on a DMG-class
            // machine, and the SGB is one.
            prev: 0x00,
            buffer: [0; BUFFER_BYTES],
            bits: 0,
            armed: false,
            receiving: false,
            complete: false,
            mask: 0,
            index: 0,
            players: 1,
        }
    }

    /// How many players the game has asked for. `1` until it sends `MLT_REQ`.
    pub fn players(&self) -> u8 {
        self.players
    }

    /// The joypad index the next read reports (0 = player 1).
    pub fn player_index(&self) -> u8 {
        self.index
    }

    /// The low nibble of `$FF00` when the game holds **both** select lines high,
    /// or `None` when the ordinary joypad answer stands (one player configured).
    pub fn player_nibble(&self) -> Option<u8> {
        (self.players > 1).then_some(0x0F - self.index)
    }

    /// Every write to `$FF00`, raw. Must be called for *all* of them: the
    /// receiver is driven entirely by the sequence of select-line states, so a
    /// dropped write corrupts a packet exactly as a dropped pulse would on
    /// hardware.
    pub fn write(&mut self, value: u8) {
        let sel = value & 0x30;
        // The joypad index advances on the rising edge of P15. A pulse that is
        // consumed as a packet bit is not special: `$10 → $30` (a `1` bit) is a
        // rising edge and does advance it, `$20 → $30` (a `0` bit) is not and
        // does not. That asymmetry is what `command_mlt_req` counts.
        if sel & 0x20 != 0 && self.prev & 0x20 == 0 {
            self.step_player();
        }

        match sel {
            // Both low: arm a transfer, and abandon anything in flight.
            0x00 => {
                self.armed = true;
                self.abandon();
            }
            // Both high: start the transfer, or latch the bit the previous
            // write selected.
            0x30 => {
                if self.armed {
                    self.armed = false;
                    self.abandon();
                    self.receiving = true;
                } else if self.receiving {
                    match self.prev {
                        0x10 => self.latch(true),
                        0x20 => self.latch(false),
                        _ => {}
                    }
                }
            }
            // One line low: the bit is not decided until the lines go high
            // again, so nothing happens here.
            _ => {}
        }
        self.prev = sel;
    }

    /// Advance the joypad index one step, wrapping within the configured player
    /// count. With one player the mask is 0 and the index can never move.
    fn step_player(&mut self) {
        self.index = (self.index + 1) & self.mask;
    }

    fn abandon(&mut self) {
        self.receiving = false;
        self.complete = false;
        self.bits = 0;
        self.buffer = [0; BUFFER_BYTES];
    }

    /// Total bits this command will occupy. The packet count is the low three
    /// bits of the first byte, and `0` means one packet (Pan Docs).
    fn command_bits(&self) -> u16 {
        let packets = (self.buffer[0] & 7).max(1) as u16;
        packets * PACKET_BITS
    }

    fn latch(&mut self, one: bool) {
        if self.complete {
            // The stop bit after a finished command. Its value does not matter —
            // the command has already run.
            self.abandon();
            return;
        }
        if self.bits as usize >= BUFFER_BYTES * 8 {
            // A command claiming more packets than exist. Drop it rather than
            // index past the buffer: a cartridge chooses this byte.
            self.abandon();
            return;
        }
        if one {
            self.buffer[(self.bits / 8) as usize] |= 1 << (self.bits % 8);
        }
        self.bits += 1;
        if self.bits == self.command_bits() {
            self.complete = true;
            self.execute();
        }
    }

    /// Act on a completed command. Everything except `MLT_REQ` is addressed to
    /// the SNES side, which this emulator does not have; ignoring it is exactly
    /// what a Game Boy with no SGB attached does, and no test ROM can tell.
    fn execute(&mut self) {
        if self.buffer[0] >> 3 != MLT_REQ {
            return;
        }
        let requested = self.buffer[1] & 3;
        self.players = requested + 1;
        self.mask = self.players - 1;
        // The SNES then polls every controller it believes is attached. For 1,
        // 2 and 4 players that is a whole number of turns of the index and
        // invisible; for the illegal count of 3 it is not, and that is the
        // "glitched player 3" `command_mlt_req` looks for.
        for _ in 0..self.players {
            self.step_player();
        }
    }

    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.prev);
        w.u16(self.bits);
        w.u8(u8::from(self.armed));
        w.u8(u8::from(self.receiving));
        w.u8(u8::from(self.complete));
        w.u8(self.mask);
        w.u8(self.index);
        w.u8(self.players);
        w.bytes(&self.buffer);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.prev = r.u8()?;
        self.bits = r.u16()?;
        self.armed = r.u8()? != 0;
        self.receiving = r.u8()? != 0;
        self.complete = r.u8()? != 0;
        self.mask = r.u8()?;
        self.index = r.u8()?;
        self.players = r.u8()?;
        r.fill(&mut self.buffer)?;
        // A state written by a future build could name a player count this
        // machine cannot represent. Clamp rather than trust it: `mask` indexes
        // nothing, but an index above it would report a player that is not
        // configured.
        self.players = self.players.clamp(1, 4);
        self.mask = self.players - 1;
        self.index &= self.mask;
        if self.bits as usize > BUFFER_BYTES * 8 {
            self.bits = 0;
            self.receiving = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pulses a game sends for one 16-byte packet: the `$00 → $30` start,
    /// each bit LSB first, then the `0` stop bit.
    fn packet(bytes: &[u8; 16]) -> Vec<u8> {
        let mut w = vec![0x00, 0x30];
        for b in bytes {
            for i in 0..8 {
                w.push(if b >> i & 1 == 1 { 0x10 } else { 0x20 });
                w.push(0x30);
            }
        }
        w.extend_from_slice(&[0x20, 0x30]);
        w
    }

    fn mlt_req(param: u8) -> Vec<u8> {
        let mut p = [0u8; 16];
        p[0] = (MLT_REQ << 3) | 1;
        p[1] = param;
        packet(&p)
    }

    fn drive(sgb: &mut Sgb, writes: &[u8]) {
        for w in writes {
            sgb.write(*w);
        }
    }

    /// `$10 → $30`, the pulse `command_mlt_req`'s `Increment` uses.
    fn increment(sgb: &mut Sgb) {
        drive(sgb, &[0x10, 0x30]);
    }

    fn nibble(sgb: &Sgb) -> u8 {
        sgb.player_nibble().unwrap_or(0x0F)
    }

    #[test]
    fn a_multiplayer_request_is_answered_in_the_joypad_register() {
        let mut sgb = Sgb::new();
        assert_eq!(sgb.players(), 1);
        assert_eq!(sgb.player_nibble(), None, "one player is not multiplayer");

        drive(&mut sgb, &mlt_req(1));
        assert_eq!(sgb.players(), 2);
        assert_eq!(nibble(&sgb), 0x0F, "reads as player 1 straight after");
        increment(&mut sgb);
        assert_eq!(nibble(&sgb), 0x0E, "then player 2");
        increment(&mut sgb);
        assert_eq!(nibble(&sgb), 0x0F, "and wraps");
    }

    #[test]
    fn four_players_cycle_in_order() {
        let mut sgb = Sgb::new();
        drive(&mut sgb, &mlt_req(3));
        for want in [0x0F, 0x0E, 0x0D, 0x0C, 0x0F] {
            assert_eq!(nibble(&sgb), want);
            increment(&mut sgb);
        }
    }

    /// The one pulse rule `command_mlt_req_1_incrementing` exists to pin: the
    /// index moves on P15's rising edge and on nothing else.
    #[test]
    fn only_a_rising_p15_edge_advances_the_player() {
        // Each case is the pulse pattern and whether the reported player moves.
        let cases: [(&[u8], bool); 8] = [
            (&[0x10, 0x30], true),
            (&[0x20, 0x30], false),
            (&[0x10, 0x20, 0x30], true),
            (&[0x10, 0x20, 0x10, 0x30], false), // two edges: back where it began
            (&[0x10, 0x10, 0x30], true),
            (&[0x00, 0x10, 0x30], true),
            (&[0x10, 0x00, 0x30], true),
            (&[0x00, 0x30], true),
        ];
        for (pattern, moves) in cases {
            let mut sgb = Sgb::new();
            drive(&mut sgb, &mlt_req(1));
            let before = nibble(&sgb);
            drive(&mut sgb, pattern);
            assert_eq!(
                nibble(&sgb) != before,
                moves,
                "pattern {pattern:02X?} should {} move the player",
                if moves { "" } else { "not" }
            );
        }
    }

    /// The illegal `MLT_REQ 2`. Three players is a real state, not a rounded-up
    /// four, and the machine alternates between players 1 and 3.
    #[test]
    fn the_illegal_three_player_request_alternates_players_one_and_three() {
        let mut sgb = Sgb::new();
        drive(&mut sgb, &mlt_req(3));
        drive(&mut sgb, &mlt_req(2));
        assert_eq!(sgb.players(), 3, "not rounded up to four");
        for _ in 0..4 {
            assert_eq!(nibble(&sgb), 0x0D, "player 3, and it stays there");
            increment(&mut sgb);
        }
    }

    /// The bit is whatever line was low **last** before the lines went high.
    #[test]
    fn the_last_line_held_low_decides_the_bit() {
        // `MLT_REQ 0` is one player; corrupt its parameter's bit 0 into a `1`
        // by pulsing `$20 → $10 → $30` and it becomes a two-player request.
        let mut sgb = Sgb::new();
        let mut w = vec![0x00, 0x30];
        let mut p = [0u8; 16];
        p[0] = (MLT_REQ << 3) | 1;
        for i in 0..8 {
            w.push(if p[0] >> i & 1 == 1 { 0x10 } else { 0x20 });
            w.push(0x30);
        }
        w.extend_from_slice(&[0x20, 0x10, 0x30]); // parameter bit 0, corrupted
        for _ in 1..8 {
            w.extend_from_slice(&[0x20, 0x30]);
        }
        for _ in 2..16 {
            for _ in 0..8 {
                w.extend_from_slice(&[0x20, 0x30]);
            }
        }
        w.extend_from_slice(&[0x20, 0x30]);
        drive(&mut sgb, &w);
        assert_eq!(sgb.players(), 2, "the $10 pulse won, so the bit was a 1");
    }

    /// A `$00` that is not followed by `$30` starts nothing.
    #[test]
    fn a_transfer_without_its_opening_pulse_is_ignored() {
        let mut sgb = Sgb::new();
        let full = mlt_req(3);
        // Drop the `$30` that acknowledges the `$00`.
        let short: Vec<u8> = full[..1].iter().chain(full[2..].iter()).copied().collect();
        drive(&mut sgb, &short);
        assert_eq!(sgb.players(), 1, "no command was received");
    }

    /// The command runs on the last bit, so a bad stop bit cannot cancel it.
    #[test]
    fn a_corrupt_stop_bit_does_not_cancel_the_command() {
        let mut sgb = Sgb::new();
        let mut w = mlt_req(3);
        let n = w.len();
        w[n - 2] = 0x10; // a `1` where the stop bit should be a `0`
        drive(&mut sgb, &w);
        assert_eq!(sgb.players(), 4);
    }

    /// A cartridge picks the packet count. Seven is the most that fits; anything
    /// claiming more must be dropped, not indexed past the buffer.
    #[test]
    fn an_overlong_command_cannot_run_off_the_buffer() {
        let mut sgb = Sgb::new();
        let mut p = [0u8; 16];
        p[0] = (MLT_REQ << 3) | 7; // the largest legal chain
        p[1] = 3;
        // Send eight packets' worth of pulses; the receiver must survive it.
        let mut w = packet(&p);
        for _ in 0..7 {
            w.extend(packet(&[0u8; 16])[2..].iter().copied());
        }
        drive(&mut sgb, &w);
        // Nothing asserted about the outcome beyond "it did not panic": a
        // seven-packet MLT_REQ is not a thing any game sends.
        let _ = sgb.players();
    }

    #[test]
    fn state_survives_a_save_and_load() {
        let mut sgb = Sgb::new();
        drive(&mut sgb, &mlt_req(3));
        increment(&mut sgb);
        let mut w = crate::savestate::Writer::new();
        sgb.save_state(&mut w);

        let mut back = Sgb::new();
        let mut r = crate::savestate::Reader::new(&w.buf);
        back.load_state(&mut r).expect("round trip");
        assert_eq!(back.players(), 4);
        assert_eq!(back.player_index(), 1);
        assert_eq!(back.player_nibble(), Some(0x0E));
    }
}

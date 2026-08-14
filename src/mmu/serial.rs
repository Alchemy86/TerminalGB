pub type SerialCallback<'a> = Box<dyn FnMut(u8) -> Option<u8> + Send + 'a>;

fn noop(_: u8) -> Option<u8> {
    None
}

/// What the link port is doing, seen from the wire rather than from the game.
///
/// This is the whole of what a cable joining two emulated consoles needs to
/// know, and it is derived from `SC` exactly as the hardware's own decode is
/// (Pan Docs, *Serial Data Transfer*: bit 7 is the transfer-start flag, bit 0
/// selects the shift clock — 1 internal, 0 external).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRole {
    /// Nothing armed. An incoming clock is ignored.
    Idle,
    /// `SC = $81`: this console is driving the shift clock and a transfer is in
    /// flight. It will finish on its own eight edges whether or not anything
    /// answers.
    Master,
    /// `SC = $80`: armed with the *external* clock selected. Nothing happens
    /// until the other console clocks it — with no cable this state persists
    /// for ever, which is exactly what the hardware does and why a lone
    /// emulator has never needed to model it.
    Slave,
}

pub struct Serial<'a> {
    data: u8,
    control: u8,
    callback: SerialCallback<'a>,
    pub interrupt: u8,
    /// A transfer the host callback declined to answer, waiting for a link
    /// partner plugin to shift a byte back.
    ///
    /// This is the seam that lets a plugin *be* the other Game Boy (see
    /// [`crate::plugin`]). It exists because a link partner needs more than the
    /// byte — it wants the machine handle — and the plugin host lives on the
    /// `Cpu`, one level above the MMU, so the reply cannot be produced from
    /// inside this write without a self-borrow. It is serviced by
    /// `Cpu::do_cycle` immediately after the instruction that wrote `SC`, which
    /// is the same point the interrupt would have been folded into `IF` anyway
    /// — so the game observes exactly the timing it did before.
    ///
    /// Transient within one instruction, therefore deliberately **not** part of
    /// the save state.
    pending: Option<u8>,
    /// Shift-clock ticks still owed before the byte has been exchanged, or 0
    /// when no internally-clocked transfer is running.
    ///
    /// A transfer is **not** instantaneous: eight falling edges of the system
    /// counter's bit 8 (8192 Hz, so ~1 ms per byte) have to go by first, and
    /// those edges are aligned to the counter, not to the `SC` write — see
    /// [`Timer::take_serial_edges`](crate::mmu::timer::Timer::take_serial_edges).
    /// Mooneye `acceptance/serial/boot_sclk_align` measures both facts from a
    /// cold boot and fails an emulator that completes on the write.
    bits_left: u8,
    /// The byte that will land in `SB` when the shift register has run out.
    /// `$FF` — an unplugged cable's idle-high data line — unless something
    /// answered.
    incoming: u8,
    /// Bumped every time an internally-clocked transfer *starts*.
    ///
    /// A [`crate::link::LinkCable`] pairs the two consoles once per transfer and
    /// must be able to tell "still the same transfer I answered" from "a second
    /// transfer that happens to look identical". Role and byte cannot
    /// distinguish those; a sequence number can, and it costs one wrapping add
    /// per `SC` write.
    ///
    /// Deliberately **not** serialised, for the same reason `pending` is not: it
    /// is meaningful only within one live link session, and a save state does
    /// not carry the other console.
    link_seq: u32,
    /// While set, an internally clocked transfer **will not finish**.
    ///
    /// A cable joining two separate processes cannot promise that the far
    /// console is even executing when our shift register runs out: both
    /// emulators pace themselves to 59.7275 Hz, and each one emulates a whole
    /// frame in a fraction of a millisecond and then sleeps for the rest of it.
    /// Two such bursts need not overlap at all, so a transfer that completed on
    /// schedule would shift in `$FF` every time and no link would ever work.
    ///
    /// Holding costs the game nothing it can observe: a program waiting on a
    /// transfer is *already* spinning on `SC` bit 7 or on the serial interrupt
    /// (`Serial_ExchangeByte`, pokered `home/serial.asm`), and the timer, the
    /// PPU and the APU all keep running underneath it. See
    /// [`crate::link::peer`].
    ///
    /// Not serialised: it is meaningful only while a peer is attached, and a
    /// save state does not carry the other console.
    link_hold: bool,
}

impl<'a> Serial<'a> {
    pub fn new_with_callback(cb: SerialCallback<'a>) -> Serial<'a> {
        Serial {
            data: 0,
            control: 0,
            callback: cb,
            interrupt: 0,
            pending: None,
            bits_left: 0,
            incoming: 0xFF,
            link_seq: 0,
            link_hold: false,
        }
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF01 => self.data = v,
            0xFF02 => {
                self.control = v;
                if v & 0x81 == 0x81 {
                    // Internal clock: this console drives the transfer. The
                    // host callback gets first refusal (that is the Blargg
                    // capture hook); if it declines, the byte is offered to a
                    // link-partner plugin via `pending`. Either way the
                    // transfer now *starts* rather than finishing here — the
                    // reply is only latched into `SB` eight shift clocks later.
                    self.incoming = match (self.callback)(self.data) {
                        Some(v) => v,
                        None => {
                            self.pending = Some(self.data);
                            // Nothing on the other end yet. A cable with no
                            // partner idles high, so an unanswered transfer
                            // still completes — with `$FF`.
                            0xFF
                        }
                    };
                    self.bits_left = 8;
                    self.link_seq = self.link_seq.wrapping_add(1);
                } else if v & 0x80 == 0 {
                    // Clearing the start bit aborts a transfer in flight.
                    self.bits_left = 0;
                }
            }
            _ => panic!("Serial does not handle address {:4X} (write)", a),
        };
    }

    /// Advance the shift register by `edges` falling edges of the 8192 Hz
    /// internal shift clock. Only an *internally* clocked transfer runs off
    /// this: with `SC` bit 0 clear the other console supplies the clock, and
    /// with no partner attached nothing ever arrives.
    pub(crate) fn shift_clock(&mut self, edges: u8) {
        if self.bits_left == 0 || edges == 0 {
            return;
        }
        if self.bits_left <= edges {
            if self.link_hold {
                // Park one bit short. The register stays "in flight", so
                // `link_role` still reports `Master` and the cable can still
                // answer it.
                self.bits_left = 1;
                return;
            }
            self.bits_left = 0;
            self.data = self.incoming;
            self.control &= 0x7F;
            self.interrupt = 0x8;
        } else {
            self.bits_left -= edges;
        }
    }

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            0xFF01 => self.data,
            0xFF02 => self.control | 0b01111110,
            _ => panic!("Serial does not handle address {:4X} (read)", a),
        }
    }

    /// Save the serial *state*. The host callback is not part of the machine
    /// state and is left untouched by a load.
    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.data);
        w.u8(self.control);
        w.u8(self.interrupt);
        w.u8(self.bits_left);
        w.u8(self.incoming);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.data = r.u8()?;
        self.control = r.u8()?;
        self.interrupt = r.u8()?;
        self.bits_left = r.u8()?;
        self.incoming = r.u8()?;
        Ok(())
    }

    /// Take the byte the game just clocked out, if a transfer is waiting for a
    /// link partner. Always call this after an instruction: the offer is made
    /// once and only once, so a partner can never answer a transfer out of
    /// order. Declining it no longer *cancels* the transfer — an unplugged
    /// cable still shifts eight bits and delivers `$FF`, which is what the
    /// hardware does and what `acceptance/serial/boot_sclk_align` requires.
    pub(crate) fn take_pending(&mut self) -> Option<u8> {
        self.pending.take()
    }

    /// A link partner shifted `received` back. It replaces the idle-high `$FF`
    /// the transfer started with; the transfer still finishes on its own shift
    /// clock, exactly as the host-callback path does.
    pub(crate) fn complete_transfer(&mut self, received: u8) {
        self.incoming = received;
        if self.bits_left == 0 {
            // Nothing in flight (the partner answered a transfer that has
            // already run out, or one it was not offered) — land it now rather
            // than losing the byte.
            self.data = received;
            self.control &= 0x7F;
            self.interrupt = 0x8;
        }
    }

    // ---------------------------------------------------------------------
    // The wire. Everything below exists for `crate::link::LinkCable`, which
    // joins two whole cores rather than impersonating one, and therefore needs
    // to see the port as the *cable* sees it rather than as the game does.
    // ---------------------------------------------------------------------

    /// What this console's link port is doing right now — see [`LinkRole`].
    ///
    /// `Master` is derived from the shift register having bits left rather than
    /// from `SC`, because those are the same thing for an internally-clocked
    /// transfer and the register is the thing that actually finishes.
    pub(crate) fn link_role(&self) -> LinkRole {
        if self.bits_left > 0 {
            LinkRole::Master
        } else if self.control & 0x81 == 0x80 {
            LinkRole::Slave
        } else {
            LinkRole::Idle
        }
    }

    /// The byte this console is holding in its shift register, i.e. the one it
    /// will put on the wire. Both sides load `SB` *before* arming `SC`, so this
    /// is stable for the whole of a transfer.
    pub(crate) fn link_out(&self) -> u8 {
        self.data
    }

    /// Which internally-clocked transfer this is, counting from power-on. The
    /// cable uses it to avoid answering one transfer twice, or mistaking a
    /// second identical-looking transfer for the one it already paired.
    pub(crate) fn link_seq(&self) -> u32 {
        self.link_seq
    }

    /// Tell a master-side transfer which byte the other console is shifting
    /// back. It replaces the idle-high `$FF` the transfer started with; the
    /// transfer still finishes on its own eight shift clocks, so the game sees
    /// hardware timing rather than an instantaneous exchange.
    pub(crate) fn link_answer_master(&mut self, received: u8) {
        self.incoming = received;
    }

    /// Stop (or allow) an internally clocked transfer finishing — see the
    /// `link_hold` field. Releasing does not itself complete the transfer; the
    /// next shift-clock edge does, which keeps the game's timing on the
    /// hardware's own clock rather than on the cable's.
    // Gated like its one caller, `link::peer` (`#[cfg(unix)]`) — dead code on
    // wasm and Windows otherwise.
    #[cfg(unix)]
    pub(crate) fn link_set_hold(&mut self, hold: bool) {
        self.link_hold = hold;
    }

    /// Clock a *slave*: the other console has driven eight edges, so this one's
    /// shift register has been emptied onto the wire and refilled with
    /// `received`. Lands the byte, clears the transfer-start flag and requests
    /// the serial interrupt — the same three effects the master sees, which is
    /// what makes the two consoles agree.
    ///
    /// Returns `false` if this console is not in fact armed for an external
    /// clock. That is a **dropped byte**: the two byte streams no longer agree
    /// and the cable must treat it as a desync rather than carry on.
    pub(crate) fn link_clock_slave(&mut self, received: u8) -> bool {
        if self.control & 0x81 != 0x80 {
            return false;
        }
        self.data = received;
        self.control &= 0x7F;
        self.interrupt = 0x8;
        true
    }

    pub fn set_callback(&mut self, cb: SerialCallback<'static>) {
        self.callback = cb;
    }

    pub fn unset_callback(&mut self) {
        self.callback = Box::new(noop);
    }
}

impl Default for Serial<'static> {
    fn default() -> Serial<'static> {
        Serial {
            data: 0,
            control: 0,
            callback: Box::new(noop),
            interrupt: 0,
            pending: None,
            bits_left: 0,
            incoming: 0xFF,
            link_seq: 0,
            link_hold: false,
        }
    }
}

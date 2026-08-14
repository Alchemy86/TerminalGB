pub type SerialCallback<'a> = Box<dyn FnMut(u8) -> Option<u8> + Send + 'a>;

fn noop(_: u8) -> Option<u8> {
    None
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
        }
    }
}

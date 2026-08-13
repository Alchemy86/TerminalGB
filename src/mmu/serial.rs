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
}

impl<'a> Serial<'a> {
    pub fn new_with_callback(cb: SerialCallback<'a>) -> Serial<'a> {
        Serial {
            data: 0,
            control: 0,
            callback: cb,
            interrupt: 0,
            pending: None,
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
                    // link-partner plugin via `pending`.
                    match (self.callback)(self.data) {
                        Some(v) => {
                            self.data = v;
                            self.interrupt = 0x8
                        }
                        None => self.pending = Some(self.data),
                    }
                }
            }
            _ => panic!("Serial does not handle address {:4X} (write)", a),
        };
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
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.data = r.u8()?;
        self.control = r.u8()?;
        self.interrupt = r.u8()?;
        Ok(())
    }

    /// Take the byte the game just clocked out, if a transfer is waiting for a
    /// link partner. Always call this after an instruction: an unanswered
    /// transfer must be dropped (an unplugged cable), not left to be answered
    /// out of order later.
    pub(crate) fn take_pending(&mut self) -> Option<u8> {
        self.pending.take()
    }

    /// A link partner shifted `received` back. Completes the transfer exactly
    /// as the host callback path does.
    pub(crate) fn complete_transfer(&mut self, received: u8) {
        self.data = received;
        self.interrupt = 0x8;
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
        }
    }
}

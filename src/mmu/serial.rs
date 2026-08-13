pub type SerialCallback<'a> = Box<dyn FnMut(u8) -> Option<u8> + Send + 'a>;

fn noop(_: u8) -> Option<u8> {
    None
}

pub struct Serial<'a> {
    data: u8,
    control: u8,
    callback: SerialCallback<'a>,
    pub interrupt: u8,
}

impl<'a> Serial<'a> {
    pub fn new_with_callback(cb: SerialCallback<'a>) -> Serial<'a> {
        Serial {
            data: 0,
            control: 0,
            callback: cb,
            interrupt: 0,
        }
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0xFF01 => self.data = v,
            0xFF02 => {
                self.control = v;
                if v & 0x81 == 0x81 {
                    if let Some(v) = (self.callback)(self.data) {
                        self.data = v;
                        self.interrupt = 0x8
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
        }
    }
}

//! [`Machine`] — a plugin's handle on the running console — and [`Bus`], the
//! narrow thing the emulator implements to provide it.
//!
//! The split matters. [`Bus`] is *unchecked*: every method does what it says.
//! [`Machine`] wraps it with the calling plugin's declared
//! [`Capability`] set and refuses anything undeclared. A plugin only ever sees
//! [`Machine`], so the enforcement cannot be bypassed by a plugin holding onto
//! the wrong object — and the emulator only ever implements [`Bus`], so it
//! cannot forget to check.

use crate::capability::Capability;
use crate::input::{KeypadKey, DEFAULT_GAP, DEFAULT_HOLD};

/// What the emulator provides so plugins can be given a [`Machine`].
///
/// **Implemented by the emulator, called only by [`Machine`].** Nothing here is
/// capability-checked; that is [`Machine`]'s job. Every method must be free of
/// side effects on emulated *timing*: a plugin looking at the machine must not
/// change the run it is looking at, which is why the peeks are documented as
/// not ticking the bus.
pub trait Bus {
    /// Read one byte of the emulated address space without ticking the bus.
    fn peek(&mut self, address: u16) -> u8;

    /// Write one byte of the emulated address space without ticking the bus.
    fn poke(&mut self, address: u16, value: u8);

    /// Read from an explicitly numbered ROM bank, whatever the game has mapped,
    /// without touching the mapper's registers. Past the end of the cartridge
    /// is open bus (`0xFF`), never a panic.
    fn peek_rom_bank(&mut self, bank: usize, address: u16) -> u8;

    /// Cartridge title from the ROM header.
    fn rom_title(&self) -> String;

    /// Header global checksum (`$014E-$014F`).
    fn global_checksum(&self) -> u16;

    /// Currently-mapped ROM bank.
    fn rom_bank(&self) -> usize;

    /// Hold or release a button on the emulated joypad.
    fn set_button(&mut self, key: KeypadKey, down: bool);

    /// Schedule a press: hold for `hold` frames, then idle for `gap` frames.
    fn queue_press(&mut self, key: KeypadKey, hold: u16, gap: u16);

    /// Abandon every scheduled press, releasing anything held.
    fn clear_input(&mut self);

    /// True when every scheduled press has been performed and released.
    fn input_idle(&self) -> bool;

    /// The picture as the LCD is showing it, RGBA, 160x144.
    fn frame_rgba(&self) -> &[u8];

    /// Record that the game has crashed — see [`Machine::report_fault`].
    fn report_fault(&mut self, reason: &str);

    /// Why the machine is considered to have crashed, if it has.
    fn fault(&self) -> Option<&str>;
}

/// A plugin's control handle on the running machine.
///
/// This is the **control** face of the extension surface, narrowed to what a
/// plugin has declared. It borrows the machine for the duration of one
/// callback, so a plugin can never stash it and poke the machine from another
/// thread.
pub struct Machine<'b> {
    bus: &'b mut dyn Bus,
    granted: Capability,
    /// Set when a plugin actually wrote memory, so the host can surface "this
    /// plugin has modified your game" in the status bar rather than only "this
    /// plugin *could*".
    pub(crate) wrote: bool,
    /// Same, for the buttons.
    pub(crate) pressed: bool,
    /// Set by [`Machine::fail`]: the plugin has decided it cannot continue and
    /// wants to be retired.
    pub(crate) failed: Option<String>,
}

impl<'b> Machine<'b> {
    /// Build a handle granting exactly `granted`. Called by the host on the way
    /// into each callback; a plugin never constructs one.
    pub fn new(bus: &'b mut dyn Bus, granted: Capability) -> Machine<'b> {
        Machine {
            bus,
            granted,
            wrote: false,
            pressed: false,
            failed: None,
        }
    }

    /// Read one byte of emulated address space. `None` if the plugin did not
    /// declare [`Capability::READ_MEMORY`].
    ///
    /// Note this is a *plugin* read, not a CPU read: it deliberately does not
    /// tick the bus, so observing memory cannot perturb timing.
    pub fn peek(&mut self, address: u16) -> Option<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return None;
        }
        Some(self.bus.peek(address))
    }

    /// Read a run of bytes. Empty if the capability was not declared.
    pub fn peek_range(&mut self, address: u16, len: usize) -> Vec<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return Vec::new();
        }
        (0..len)
            .map(|i| self.bus.peek(address.wrapping_add(i as u16)))
            .collect()
    }

    /// Fill `out` from `base`, wrapping at the top of the address space.
    /// Allocation-free, so a plugin can sample a party structure every frame
    /// without churning the heap. Leaves `out` untouched without
    /// [`Capability::READ_MEMORY`].
    pub fn peek_into(&mut self, base: u16, out: &mut [u8]) {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return;
        }
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.bus.peek(base.wrapping_add(i as u16));
        }
    }

    /// Little-endian 16-bit read — the bus's own byte order.
    pub fn peek_u16(&mut self, address: u16) -> Option<u16> {
        let lo = self.peek(address)? as u16;
        let hi = self.peek(address.wrapping_add(1))? as u16;
        Some(lo | (hi << 8))
    }

    /// Big-endian 16-bit read. The bus is little-endian but game data
    /// frequently is not — every Gen 1 Pokémon HP and stat is big-endian.
    pub fn peek_be16(&mut self, address: u16) -> Option<u16> {
        let hi = self.peek(address)? as u16;
        let lo = self.peek(address.wrapping_add(1))? as u16;
        Some((hi << 8) | lo)
    }

    /// Read from an **explicitly numbered ROM bank**, whatever the game
    /// currently has mapped.
    ///
    /// Cartridge data — sprite tables, name tables, base stats — lives in banks
    /// the game maps only while it is using them, so a plugin polling once a
    /// second would otherwise read whatever happened to be mapped. `address` is
    /// used modulo the 16 KiB bank window, so both `$0000` and `$4000` forms
    /// work. Reads past the end of the cartridge return `0xFF` (open bus),
    /// never a panic.
    ///
    /// It does not switch banks: the mapper's registers are untouched, so this
    /// cannot perturb the running game the way a read-then-restore would.
    pub fn peek_bank(&mut self, bank: usize, address: u16) -> Option<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return None;
        }
        Some(self.bus.peek_rom_bank(bank, address))
    }

    /// Cartridge title from the ROM header.
    pub fn rom_title(&self) -> String {
        self.bus.rom_title()
    }

    /// Header global checksum (`$014E-$014F`) — enough to tell one build of a
    /// game from another, which is how a game-specific plugin refuses to
    /// display garbage for a ROM it does not know.
    pub fn global_checksum(&self) -> u16 {
        self.bus.global_checksum()
    }

    /// Currently-mapped ROM bank.
    pub fn rom_bank(&self) -> usize {
        self.bus.rom_bank()
    }

    /// Write one byte. Returns `false` (and does nothing) if the plugin did not
    /// declare [`Capability::WRITE_MEMORY`].
    ///
    /// **This can corrupt a save.** The virtual link partner shipped with the
    /// emulator deliberately does not declare this capability: a traded Pokémon
    /// must arrive through the real trade path, not through a poke.
    pub fn poke(&mut self, address: u16, value: u8) -> bool {
        if !self.granted.contains(Capability::WRITE_MEMORY) {
            return false;
        }
        self.bus.poke(address, value);
        self.wrote = true;
        true
    }

    /// Hold or release one of the console's buttons. Returns `false` (and does
    /// nothing) without [`Capability::CONTROL_INPUT`].
    ///
    /// Pressing buttons is a **separate** capability from
    /// [`Capability::WRITE_MEMORY`] on purpose: pressing buttons is something
    /// the game itself sanctions — every byte it changes, it changes through
    /// its own code — whereas a poke is not. A plugin that scripts a cinematic
    /// wants the first and should not have to ask for the second.
    ///
    /// Buttons are edge-triggered by the emulated joypad, so a plugin holds a
    /// button by calling this with `true` on one frame and `false` on a later
    /// one; the game samples the pad once per frame, so one frame held is one
    /// press seen. The frontend's own key handling writes the same latch, so a
    /// human pressing the same button during a scripted sequence fights it —
    /// which is why a plugin driving input must be visible in the status bar.
    pub fn set_button(&mut self, key: KeypadKey, down: bool) -> bool {
        if !self.granted.contains(Capability::CONTROL_INPUT) {
            return false;
        }
        self.bus.set_button(key, down);
        self.pressed = true;
        true
    }

    /// Schedule a button press: hold `key` down for `hold` emulated frames,
    /// then leave the pad idle for `gap` frames. Returns `false` (and does
    /// nothing) without [`Capability::CONTROL_INPUT`].
    ///
    /// The level above [`Machine::set_button`], and the one to reach for unless
    /// a plugin is genuinely thinking frame by frame: presses have to be
    /// *scheduled* in frames, because that is the granularity the game samples
    /// input at, and every caller keeping its own timer is how one mechanism
    /// quietly becomes three.
    pub fn press(&mut self, key: KeypadKey, hold: u16, gap: u16) -> bool {
        if !self.granted.contains(Capability::CONTROL_INPUT) {
            return false;
        }
        self.bus.queue_press(key, hold, gap);
        self.pressed = true;
        true
    }

    /// Schedule a plain tap with the default hold and gap.
    pub fn tap(&mut self, key: KeypadKey) -> bool {
        self.press(key, DEFAULT_HOLD, DEFAULT_GAP)
    }

    /// True when every scheduled press has been performed and released, i.e.
    /// the machine has caught up with what was asked of it. Readable without
    /// [`Capability::CONTROL_INPUT`] — knowing the pad is quiet is an
    /// observation, not an action.
    pub fn input_idle(&self) -> bool {
        self.bus.input_idle()
    }

    /// Abandon every scheduled press, releasing anything held. Refused without
    /// [`Capability::CONTROL_INPUT`].
    pub fn clear_input(&mut self) -> bool {
        if !self.granted.contains(Capability::CONTROL_INPUT) {
            return false;
        }
        self.bus.clear_input();
        true
    }

    /// The picture as the LCD is showing it, RGBA, 160x144. Needs
    /// [`Capability::OBSERVE`] — it is a look at the machine, and the most
    /// expensive one available, so a plugin that wants it says so.
    ///
    /// Structure beats pixels for almost everything, but a frame is the only
    /// honest answer when there is no address for what you need to know.
    pub fn frame_rgba(&self) -> Option<&[u8]> {
        if !self.granted.contains(Capability::OBSERVE) {
            return None;
        }
        Some(self.bus.frame_rgba())
    }

    /// Which capabilities this handle actually carries.
    pub fn granted(&self) -> Capability {
        self.granted
    }

    /// **Report that the game has crashed.** Stops the battery save being
    /// written for the rest of the session, and does nothing else.
    ///
    /// This needs **no capability**, deliberately. Every other entry point here
    /// is gated because it lets a plugin *do* something to the machine; this one
    /// only withdraws the emulator's own right to overwrite a file, so the
    /// fail-safe direction is to let any plugin call it. A plugin that watches a
    /// game closely enough to recognise an impossible state — a link partner
    /// seeing a state variable take a value the game never assigns, say — knows
    /// sooner than any CPU-level detector can.
    pub fn report_fault(&mut self, reason: &str) {
        self.bus.report_fault(reason);
    }

    /// Why the machine is considered to have crashed, if it has.
    pub fn fault(&self) -> Option<&str> {
        self.bus.fault()
    }

    /// **Retire this plugin.** The host stops dispatching to it and shows
    /// `reason` beside its name, and the emulator carries on.
    ///
    /// This is the graceful half of fault isolation: a plugin that discovers it
    /// cannot continue — a protocol desync it cannot recover from, a table it
    /// cannot find — says so instead of returning wrong answers forever. The
    /// other half is involuntary: a plugin that *panics* is caught and retired
    /// the same way (see [`PluginHost`](crate::PluginHost)).
    pub fn fail(&mut self, reason: impl Into<String>) {
        self.failed = Some(reason.into());
    }
}

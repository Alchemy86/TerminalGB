//! A plugin's control handle on the running machine, and the seam the emulator
//! implements to provide it.

use crate::{Capability, KeypadKey};

/// **What the emulator must provide.** The host side of the boundary.
///
/// A plugin never sees this trait's methods directly — it sees [`Machine`],
/// which wraps one of these and applies the capability checks. The split is
/// what makes this crate independent of the emulator: `terminalgb` implements
/// `MachineAccess` for its memory unit, and plugin crates depend only on this
/// crate. Without it the dependency graph would be a cycle, because the
/// emulator's own frontend draws panels for the plugins it ships with.
///
/// # Implementing it
///
/// Only the emulator implements this, and it already does. It is public
/// because a *test* for a plugin wants a stand-in machine — see
/// `plugins/example-tracer` for a fake implementation used to unit-test a
/// plugin with no emulator involved. That is the second reason the trait
/// exists: a plugin becomes testable without booting a Game Boy.
///
/// Every method is infallible and ungated. Refusal is [`Machine`]'s job, so an
/// implementor never has to reason about capabilities.
pub trait MachineAccess {
    /// One byte of emulated address space, without ticking the bus.
    fn peek(&mut self, address: u16) -> u8;
    /// Write one byte, without ticking the bus.
    fn poke(&mut self, address: u16, value: u8);
    /// A byte from an explicitly numbered ROM bank, whatever is mapped.
    /// `0xFF` (open bus) past the end of the cartridge — never a panic.
    fn peek_bank(&mut self, bank: usize, address: u16) -> u8;
    /// A byte from an explicitly numbered 8 KiB cartridge-RAM bank, whatever
    /// the game has mapped and whether or not it currently has RAM enabled.
    /// `None` when the cartridge has no RAM there — a reader of save data must
    /// be able to tell absent storage from a stored `$FF`, which is why this
    /// is not open bus. Default `None` so a fake machine that never deals in
    /// cartridge RAM needs no code.
    fn peek_sram_bank(&mut self, _bank: usize, _address: u16) -> Option<u8> {
        None
    }
    /// Cartridge title from the ROM header.
    fn rom_title(&self) -> String;
    /// Header global checksum (`$014E-$014F`).
    fn global_checksum(&self) -> u16;
    /// Currently-mapped ROM bank.
    fn rom_bank(&self) -> usize;
    /// Hold or release a button.
    fn set_button(&mut self, key: KeypadKey, down: bool);
    /// Schedule a press of `key` for `hold` frames followed by `gap` idle frames.
    fn queue_press(&mut self, key: KeypadKey, hold: u16, gap: u16);
    /// True when every scheduled press has been performed and released.
    fn input_idle(&self) -> bool;
    /// Abandon every scheduled press, releasing anything held.
    fn clear_input(&mut self);
    /// The picture as the LCD is showing it, RGBA, 160x144.
    fn frame_rgba(&self) -> &[u8];
    /// The same picture as a compressed PNG. Provided by the emulator because
    /// it already carries an encoder; a plugin should not grow an image
    /// dependency to hand a screenshot to something outside the process.
    fn frame_png(&self) -> Vec<u8>;
    /// Record that the game has crashed, so the battery save is not written.
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
pub struct Machine<'m> {
    inner: &'m mut dyn MachineAccess,
    granted: Capability,
    /// Set when a plugin actually wrote memory, so the host can surface "this
    /// plugin has modified your game" in the status bar rather than only "this
    /// plugin *could*".
    wrote: bool,
    /// Same, for button presses: "something else is playing my game" must never
    /// be invisible.
    pressed: bool,
}

impl<'m> Machine<'m> {
    /// Wrap a machine, granting exactly `granted`.
    ///
    /// The host calls this; it is public so a plugin's own tests can build one
    /// over a fake [`MachineAccess`] and assert on what the plugin does.
    pub fn new(inner: &'m mut dyn MachineAccess, granted: Capability) -> Machine<'m> {
        Machine {
            inner,
            granted,
            wrote: false,
            pressed: false,
        }
    }

    /// Whether this handle has been used to write memory.
    pub fn wrote(&self) -> bool {
        self.wrote
    }

    /// Whether this handle has been used to press a button.
    pub fn pressed(&self) -> bool {
        self.pressed
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
        Some(self.inner.peek(address))
    }

    /// Read a run of bytes. Empty if the capability was not declared.
    pub fn peek_range(&mut self, address: u16, len: usize) -> Vec<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return Vec::new();
        }
        (0..len)
            .map(|i| self.inner.peek(address.wrapping_add(i as u16)))
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
            *slot = self.inner.peek(base.wrapping_add(i as u16));
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
    /// It does not switch banks: the MBC's registers are untouched, so this
    /// cannot perturb the running game the way a read-then-restore would.
    pub fn peek_bank(&mut self, bank: usize, address: u16) -> Option<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return None;
        }
        Some(self.inner.peek_bank(bank, address))
    }

    /// Read from an **explicitly numbered 8 KiB cartridge-RAM bank**, whatever
    /// the game has mapped — and whether or not the game currently has RAM
    /// enabled, which it normally does not: save data (Gen 1's PC boxes live
    /// in SRAM banks 2 and 3) is unreachable through [`Machine::peek`] almost
    /// all of the time. Like [`Machine::peek_bank`] this touches no MBC
    /// register, so it cannot perturb the running game.
    ///
    /// `None` without [`Capability::READ_MEMORY`], and `None` where the
    /// cartridge has no RAM — absent storage must be distinguishable from a
    /// stored `$FF`, because the caller is reading a save.
    pub fn peek_sram_bank(&mut self, bank: usize, address: u16) -> Option<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return None;
        }
        self.inner.peek_sram_bank(bank, address)
    }

    /// Cartridge title from the ROM header.
    pub fn rom_title(&self) -> String {
        self.inner.rom_title()
    }

    /// Header global checksum (`$014E-$014F`) — enough to tell one build of a
    /// game from another, which is how a game-specific plugin refuses to display
    /// garbage for a ROM it does not know.
    pub fn global_checksum(&self) -> u16 {
        self.inner.global_checksum()
    }

    /// Currently-mapped ROM bank.
    pub fn rom_bank(&self) -> usize {
        self.inner.rom_bank()
    }

    /// Write one byte. Returns `false` (and does nothing) if the plugin did not
    /// declare [`Capability::WRITE_MEMORY`].
    ///
    /// **This can corrupt a save.** The virtual link partner shipped in
    /// `terminalgb-plugin-gen1-link` deliberately does not declare this
    /// capability: a traded Pokémon must arrive through the real trade path,
    /// not through a poke.
    pub fn poke(&mut self, address: u16, value: u8) -> bool {
        if !self.granted.contains(Capability::WRITE_MEMORY) {
            return false;
        }
        self.inner.poke(address, value);
        self.wrote = true;
        true
    }

    /// Hold or release one of the console's buttons. Returns `false` (and does
    /// nothing) without [`Capability::CONTROL_INPUT`].
    ///
    /// This is the joypad half of the *control* face `docs/debugging.md`
    /// specifies, and it is a **separate** capability from
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
        self.inner.set_button(key, down);
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
        self.inner.queue_press(key, hold, gap);
        self.pressed = true;
        true
    }

    /// Schedule a plain tap with the default hold and gap.
    pub fn tap(&mut self, key: KeypadKey) -> bool {
        self.press(key, crate::DEFAULT_HOLD, crate::DEFAULT_GAP)
    }

    /// True when every scheduled press has been performed and released, i.e.
    /// the machine has caught up with what was asked of it. Readable without
    /// [`Capability::CONTROL_INPUT`] — knowing the pad is quiet is an
    /// observation, not an action.
    pub fn input_idle(&self) -> bool {
        self.inner.input_idle()
    }

    /// Abandon every scheduled press, releasing anything held. Refused without
    /// [`Capability::CONTROL_INPUT`].
    pub fn clear_input(&mut self) -> bool {
        if !self.granted.contains(Capability::CONTROL_INPUT) {
            return false;
        }
        self.inner.clear_input();
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
        Some(self.inner.frame_rgba())
    }

    /// The same picture as a compressed PNG, for handing to something outside
    /// the process. Needs [`Capability::OBSERVE`].
    pub fn frame_png(&self) -> Option<Vec<u8>> {
        if !self.granted.contains(Capability::OBSERVE) {
            return None;
        }
        Some(self.inner.frame_png())
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
    /// seeing `wLinkState` take a value the game never assigns, say — knows
    /// sooner than any CPU-level detector can.
    pub fn report_fault(&mut self, reason: &str) {
        self.inner.report_fault(reason);
    }

    /// Why the machine is considered to have crashed, if it has.
    pub fn fault(&self) -> Option<&str> {
        self.inner.fault()
    }
}

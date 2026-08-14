//! Test support for plugin authors. Behind the `testing` feature, off by
//! default.
//!
//! A plugin crate depends on `terminalgb-plugin-api`, not on this emulator, so its
//! unit tests normally run against a fake [`MachineAccess`] — see
//! `plugins/example-tracer`, which needs no emulator at all. But some plugins
//! read *cartridge* structure: banked ROM tables, cartridge RAM, an MBC that
//! actually switches banks. A fake that models all of that faithfully is a
//! second emulator, and a worse one.
//!
//! [`TestMachine`] is the honest alternative: a real machine, exposed only as
//! the [`MachineAccess`] a plugin already talks to. Use it as a
//! **dev-dependency**, which is also the only direction Cargo permits, since
//! the emulator depends on the plugin crates it ships:
//!
//! ```toml
//! [dev-dependencies]
//! terminalgb = { version = "0.2", default-features = false, features = ["testing"] }
//! ```
//!
//! ```no_run
//! # use terminalgb::testing::TestMachine;
//! # use terminalgb_plugin_api::{PluginHost, MachineAccess};
//! let mut machine = TestMachine::new(my_rom()).unwrap();
//! let mut host = PluginHost::new();
//! host.register(Box::new(MyPlugin::new())).unwrap();
//! for _ in 0..60 {
//!     host.on_frame(&mut machine);
//! }
//! assert_eq!(host.statuses()[0].detail, "…");
//! # fn my_rom() -> Vec<u8> { vec![] }
//! # struct MyPlugin; impl MyPlugin { fn new() -> Self { MyPlugin } }
//! # impl terminalgb_plugin_api::Plugin for MyPlugin {
//! #   fn name(&self) -> &str { "" }
//! #   fn capabilities(&self) -> terminalgb_plugin_api::Capability { Default::default() }
//! #   fn as_any(&self) -> &dyn std::any::Any { self }
//! #   fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self } }
//! ```
//!
//! Note what this deliberately does **not** hand out: the memory unit, the CPU,
//! or any other emulator internal. A plugin that cannot be tested through
//! `MachineAccess` is a plugin reaching for something the contract does not
//! offer, and that is worth discovering in a test rather than in a refactor.

use crate::mmu::MemoryManagementUnit;
use terminalgb_plugin_api::MachineAccess;

/// A real emulated machine, borrowable as a [`MachineAccess`].
///
/// It is not running: nothing steps the CPU, so the state is exactly what the
/// test puts there. That is usually what a plugin test wants — drive
/// `PluginHost::on_frame` yourself and play the part of the game in between.
pub struct TestMachine {
    mem: MemoryManagementUnit<'static>,
}

impl TestMachine {
    /// Build a machine from a cartridge image. `Err` for a ROM the MBC layer
    /// refuses (a bad header, an unsupported cartridge type).
    ///
    /// No filesystem path is given, so nothing is ever written: a plugin test
    /// cannot create or destroy a battery save.
    pub fn new(rom: Vec<u8>) -> Result<TestMachine, &'static str> {
        MemoryManagementUnit::new(rom, None).map(|mem| TestMachine { mem })
    }

    /// Read as the *running game* would: the ordinary bus path, with MBC
    /// banking and I/O read-back, not the observer's `peek`.
    ///
    /// Plugins deliberately have no such call — observation must not perturb
    /// the run. A test, though, often needs to play the part of the game, and
    /// this is that: enable cartridge RAM, poke a hardware register, check what
    /// the game would see.
    pub fn bus_read(&mut self, address: u16) -> u8 {
        self.mem.rb(address)
    }

    /// Write as the running game would — see [`TestMachine::bus_read`].
    pub fn bus_write(&mut self, address: u16, value: u8) {
        self.mem.wb(address, value)
    }

    /// Advance the scheduled-press queue by one frame, as the frame loop does.
    /// A test that asserts a plugin's button press reached the joypad needs
    /// this, because nothing else is stepping the machine.
    pub fn advance_input(&mut self) {
        self.mem.advance_input()
    }
}

impl MachineAccess for TestMachine {
    fn peek(&mut self, address: u16) -> u8 {
        self.mem.peek(address)
    }
    fn poke(&mut self, address: u16, value: u8) {
        self.mem.poke(address, value)
    }
    fn peek_bank(&mut self, bank: usize, address: u16) -> u8 {
        self.mem.peek_bank(bank, address)
    }
    fn rom_title(&self) -> String {
        self.mem.rom_title()
    }
    fn global_checksum(&self) -> u16 {
        self.mem.global_checksum()
    }
    fn rom_bank(&self) -> usize {
        self.mem.rom_bank()
    }
    fn set_button(&mut self, key: crate::input::KeypadKey, down: bool) {
        self.mem.set_button(key, down)
    }
    fn queue_press(&mut self, key: crate::input::KeypadKey, hold: u16, gap: u16) {
        self.mem.queue_press(key, hold, gap)
    }
    fn input_idle(&self) -> bool {
        self.mem.input_idle()
    }
    fn clear_input(&mut self) {
        self.mem.clear_input()
    }
    fn frame_rgba(&self) -> &[u8] {
        self.mem.frame_rgba()
    }
    fn frame_png(&self) -> Vec<u8> {
        self.mem.frame_png()
    }
    fn report_fault(&mut self, reason: &str) {
        MachineAccess::report_fault(&mut self.mem, reason)
    }
    fn fault(&self) -> Option<&str> {
        MachineAccess::fault(&self.mem)
    }
}

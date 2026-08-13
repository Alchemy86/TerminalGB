//! The plugin surface: read-only inspection plus a frame event.
//!
//! This is the *consumer* end of the extension surface designed in
//! [`docs/debugging.md`](../../docs/debugging.md) §"The public extension
//! surface — emit, control, transport". It deliberately implements only the two
//! pieces a **read-only** plugin needs, in the shape that document specifies:
//!
//! * **Tier 1, read-only inspector** — [`DebugView`], a borrowing view built
//!   from `&Gameboy` with side-effect-free reads. Because it is `&self` and only
//!   reads, it costs nothing unless a plugin calls it, so it is not
//!   feature-gated.
//! * **Emit, "frame rendered"** — the cheap, ~60/s event the design calls "the
//!   general capture primitive". Plugins are driven from the frontend's loop via
//!   [`PluginHost::frame`]; there is no per-instruction or per-access hook here,
//!   so the hot path (`do_cycle` → `operation` → `mmu::rb`/`wb`) is untouched.
//!
//! Control (`step`/`run_until`/`poke`) and the hot event categories are **not**
//! implemented here — a tracker must not be able to write to the machine, and
//! the [`Plugin`] trait is given `&DebugView`, never `&mut Gameboy`, so it
//! cannot. See [`crate::trainer`] for the first plugin.
//!
//! ## Notes for the plugin-system crew
//!
//! Two deltas this consumer needed against the sketch in `docs/debugging.md`,
//! recorded because a second consumer is the only way to find them:
//!
//! 1. `DebugSink::on_events(&mut self, batch: &[Event])` has **no view
//!    parameter**. A plugin that reacts to an event by *reading state* — which
//!    is what a tracker does, and probably what most plugins do — cannot. The
//!    signature here passes the view alongside the events.
//! 2. There is **no banked-ROM read**. `peek` sees only the currently-mapped
//!    bank, so a plugin that wants cartridge data (sprite/graphics tables, text
//!    tables) cannot get at it without forcing a bank switch, which is a write.
//!    Until the contract grows a `peek_bank(bank, addr)`, the frontend hands the
//!    plugin the cartridge image directly; see [`PluginHost::new`].

use crate::gameboy::Gameboy;

/// Something the machine did, published to plugins.
///
/// Only the cheap end of the event table in `docs/debugging.md` is modelled.
/// The two ruinous categories (instruction-retired, memory-access) are absent
/// by design: they belong behind the `debug` cargo feature and a ring buffer,
/// and nothing here needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A frame finished emulating. `index` counts frames since the session
    /// started, so a plugin can sample at its own cadence rather than reading
    /// memory sixty times a second.
    Frame { index: u64 },
}

/// Read-only snapshot access to a running machine.
///
/// Holds `&Gameboy`, so a plugin holding one is *statically* incapable of
/// mutating the emulator — the read-only guarantee is the borrow checker's, not
/// a convention.
pub struct DebugView<'a> {
    gb: &'a Gameboy,
}

impl<'a> DebugView<'a> {
    pub(crate) fn new(gb: &'a Gameboy) -> DebugView<'a> {
        DebugView { gb }
    }

    /// Side-effect-free bus read. See [`crate::mmu::MemoryManagementUnit::peek`]
    /// for the one caveat: the I/O range `$FF00-$FF7F` reads back `0xFF`.
    pub fn peek(&self, addr: u16) -> u8 {
        self.gb.peek(addr)
    }

    /// Fill `out` with `out.len()` bytes starting at `base`, wrapping at the
    /// top of the address space. Allocation-free, so a plugin can read a party
    /// structure per second without churning the heap.
    pub fn peek_into(&self, base: u16, out: &mut [u8]) {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.peek(base.wrapping_add(i as u16));
        }
    }

    /// Little-endian 16-bit read.
    pub fn peek_u16(&self, addr: u16) -> u16 {
        (self.peek(addr) as u16) | ((self.peek(addr.wrapping_add(1)) as u16) << 8)
    }

    /// Big-endian 16-bit read. The Game Boy bus is little-endian but game data
    /// frequently is not — Gen 1 Pokémon stores every HP and stat big-endian.
    pub fn peek_be16(&self, addr: u16) -> u16 {
        ((self.peek(addr) as u16) << 8) | self.peek(addr.wrapping_add(1)) as u16
    }

    /// Cartridge title from the ROM header.
    pub fn rom_title(&self) -> String {
        self.gb.rom_title()
    }

    /// Header global checksum (bytes `$014E-$014F`) — enough to tell one build
    /// of a game from another, which is how a game-specific plugin refuses to
    /// display garbage for a ROM it does not know.
    pub fn global_checksum(&self) -> u16 {
        self.gb.global_checksum()
    }

    /// Program counter, for a plugin that wants to know where the game is.
    pub fn pc(&self) -> u16 {
        self.gb.debug_pc()
    }

    /// Currently-mapped ROM bank.
    pub fn rom_bank(&self) -> usize {
        self.gb.debug_rom_bank()
    }
}

/// A read-only observer of a running machine.
///
/// Implementors are driven by [`PluginHost`]; they never see `&mut Gameboy`.
pub trait Plugin {
    /// Stable identifier, for status lines and diagnostics.
    fn name(&self) -> &'static str;

    /// Called once when the plugin is attached to a machine. The usual job is
    /// to decide whether this plugin understands this cartridge at all.
    fn on_attach(&mut self, _view: &DebugView<'_>) {}

    /// Called with the events since the last call, plus a view to read from.
    ///
    /// Called from the frontend's loop, so this must be cheap. A plugin that
    /// wants to read a lot of memory should throttle on the frame index rather
    /// than reading on every call.
    fn on_events(&mut self, events: &[Event], view: &DebugView<'_>);

    /// Downcast hook, so a frontend can recover the concrete plugin it attached
    /// in order to render it. An explicit method rather than a `dyn Any`
    /// upcast: trait upcasting is not stable on this crate's pinned 1.83
    /// toolchain (`rust-toolchain`).
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable counterpart of [`Plugin::as_any`], so a frontend can configure
    /// the plugin it attached.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Owns the attached plugins and dispatches events to them.
///
/// The host is the frontend's; it holds the cartridge image because
/// [`DebugView`] cannot yet read a bank that is not mapped (see the module
/// note). A host with no plugins costs one `is_empty` test per frame.
pub struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
    frame: u64,
    /// Scratch, reused so dispatch allocates nothing per frame.
    events: Vec<Event>,
}

impl PluginHost {
    /// A host with no plugins attached.
    pub fn new() -> PluginHost {
        PluginHost {
            plugins: Vec::new(),
            frame: 0,
            events: Vec::new(),
        }
    }

    pub fn attach(&mut self, plugin: Box<dyn Plugin>, gb: &Gameboy) {
        let mut plugin = plugin;
        plugin.on_attach(&DebugView::new(gb));
        self.plugins.push(plugin);
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Publish a "frame rendered" event. Returns immediately when nothing is
    /// attached, which is the default in every frontend.
    pub fn frame(&mut self, gb: &Gameboy) {
        self.frame += 1;
        if self.plugins.is_empty() {
            return;
        }
        self.events.clear();
        self.events.push(Event::Frame { index: self.frame });
        let view = DebugView::new(gb);
        for p in self.plugins.iter_mut() {
            p.on_events(&self.events, &view);
        }
    }

    /// Borrow an attached plugin by concrete type, so a frontend can render it.
    pub fn get<T: Plugin + 'static>(&self) -> Option<&T> {
        self.plugins
            .iter()
            .find_map(|p| p.as_any().downcast_ref::<T>())
    }

    /// Mutable borrow of an attached plugin, for the frontend to configure it
    /// (whether its view is visible, say). Plugins still never receive the
    /// machine mutably — this is the frontend talking to its own plugin, not to
    /// the emulator.
    pub fn get_mut<T: Plugin + 'static>(&mut self) -> Option<&mut T> {
        self.plugins
            .iter_mut()
            .find_map(|p| p.as_any_mut().downcast_mut::<T>())
    }
}

impl Default for PluginHost {
    fn default() -> PluginHost {
        PluginHost::new()
    }
}

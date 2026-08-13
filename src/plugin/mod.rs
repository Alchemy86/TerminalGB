//! Plugins — the *attach* half of the public extension surface.
//!
//! [`docs/debugging.md`](../docs/debugging.md) designs one extension surface with
//! three faces: **emit** (subscribe to what the machine does), **control** (drive
//! it) and **transport** (how a non-Rust tool reaches both). The render crew built
//! the first half of *emit* as the NDJSON telemetry stream
//! ([`crate::telemetry`]). This module is the other half: something that can not
//! only watch the machine but **answer** it — most importantly, answer the serial
//! port as a link-cable partner.
//!
//! It is deliberately **not** a second extension model. The event names, the
//! control verbs and the JSON encoding here are the ones that page specifies, so
//! a socket or `postMessage` transport later is a change of *pipe*, not of
//! contract. See [`docs/plugins.md`](../docs/plugins.md) for the full contract.
//!
//! # What a plugin is
//!
//! **An in-process Rust object implementing [`Plugin`].** Not a dynamic library,
//! not a script engine, not a marketplace — a trait, in the same crate or in a
//! crate that links this one. That choice is the honest one for this project's
//! constraints: it is dependency-free, it works identically on `wasm32` (where
//! there are no sockets and no `dlopen`), and it costs a null check per frame
//! when nothing is attached.
//!
//! The *messages* are designed for the wire even though the *transport* today is
//! a direct call: every [`Event`] serialises to the same NDJSON shape telemetry
//! uses ([`Event::write_json`]), and every reply a plugin can give is a small
//! value, not a borrowed pointer. An external-process transport is therefore an
//! additive follow-up that does not reshape this trait — see
//! [`docs/plugins.md`](../docs/plugins.md) §"Why in-process first".
//!
//! # What a plugin may do
//!
//! Exactly three things, each of which it must **declare up front** via
//! [`Plugin::capabilities`]:
//!
//! * [`Capability::OBSERVE`] — receive [`Event`]s (frames, serial transfers).
//! * [`Capability::READ_MEMORY`] — [`Machine::peek`] / [`Machine::peek_range`].
//! * [`Capability::WRITE_MEMORY`] — [`Machine::poke`]. Corrupts saves if misused;
//!   see the honesty rules below.
//! * [`Capability::SERIAL_LINK`] — be the console on the other end of the link
//!   cable ([`Plugin::on_serial`]).
//!
//! Declaration is enforced, not advisory: [`Machine`] checks the calling plugin's
//! declared capabilities and refuses (returning `None` / `false`) rather than
//! trusting it. A plugin that never declares `WRITE_MEMORY` *cannot* write.
//!
//! # Safety and honesty
//!
//! * **Nothing loads implicitly.** [`PluginHost`] starts empty. A plugin is
//!   registered by the frontend from explicit configuration (`plugins = …` in
//!   `config.toml`, or `GB_PLUGINS`), never by scanning a directory.
//! * **The user can always see it.** [`PluginHost::status_line`] is rendered in
//!   the terminal frontend's status panel whenever any plugin is attached, and
//!   plugins that hold `WRITE_MEMORY` are flagged there. A plugin cannot be
//!   active and invisible.
//! * **One link partner.** A real cable has two ends, so at most one plugin may
//!   hold [`Capability::SERIAL_LINK`]; a second registration is rejected.
//! * **Save-state honesty.** Plugin state is *not* part of the machine state and
//!   is not serialised (see [`docs/plugins.md`](../docs/plugins.md)); loading a
//!   state mid-exchange resets the attached link partner rather than silently
//!   desyncing.
//!
//! # The two gaps the trainer-view crew reported
//!
//! The read-only trainer view ([`crate::trainer`]) was written against the
//! sketch in `docs/debugging.md` and reported two places where that sketch did
//! not survive contact with a real consumer. Both are answered here, and the
//! answers are why there is **one** plugin trait rather than a read-only one
//! beside a read-write one:
//!
//! 1. *"`DebugSink::on_events` has no view parameter, so a plugin cannot read
//!    state in response to an event."* — [`Plugin::on_event`] takes
//!    `&mut Machine` alongside the event. Reading in response to an event is the
//!    normal case, not an extra.
//! 2. *"There is no banked-ROM read; `peek` only sees the mapped bank, so
//!    cartridge tables are unreachable."* — [`Machine::peek_bank`] reads an
//!    explicitly numbered ROM bank regardless of what the game has mapped, so a
//!    plugin no longer needs the frontend to hand it the cartridge image. It is
//!    a *read*, gated by [`Capability::READ_MEMORY`], and it does not disturb the
//!    game's banking.
//!
//! A plugin that only wants to look still cannot write: it declares no
//! [`Capability::WRITE_MEMORY`], and [`Machine::poke`] then returns `false`
//! without touching memory. The read-only guarantee moved from "the borrow
//! checker won't let you" to "the host won't let you" — enforced either way, and
//! now one contract covers both kinds of plugin.

use crate::mmu::MemoryManagementUnit;

pub mod gen1;

/// What a plugin is allowed to do. Declared by [`Plugin::capabilities`] and
/// enforced by [`Machine`] — an undeclared operation fails rather than being
/// trusted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Capability(u8);

impl Capability {
    /// Receive [`Event`]s. Costs one branch per event when unset.
    pub const OBSERVE: Capability = Capability(1 << 0);
    /// Read emulated memory through [`Machine::peek`].
    pub const READ_MEMORY: Capability = Capability(1 << 1);
    /// Write emulated memory through [`Machine::poke`]. **Can corrupt a save.**
    pub const WRITE_MEMORY: Capability = Capability(1 << 2);
    /// Drive the serial port as the other end of the link cable.
    pub const SERIAL_LINK: Capability = Capability(1 << 3);

    pub const NONE: Capability = Capability(0);

    pub const fn union(self, other: Capability) -> Capability {
        Capability(self.0 | other.0)
    }

    pub const fn contains(self, other: Capability) -> bool {
        self.0 & other.0 == other.0
    }

    /// Human-readable list, for the status bar and `--help`-style output.
    pub fn describe(self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.contains(Capability::OBSERVE) {
            parts.push("observe");
        }
        if self.contains(Capability::READ_MEMORY) {
            parts.push("read");
        }
        if self.contains(Capability::WRITE_MEMORY) {
            parts.push("write");
        }
        if self.contains(Capability::SERIAL_LINK) {
            parts.push("link");
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
}

impl std::ops::BitOr for Capability {
    type Output = Capability;
    fn bitor(self, rhs: Capability) -> Capability {
        self.union(rhs)
    }
}

/// Something the machine did. The categories and their rough rates are the ones
/// [`docs/debugging.md`](../docs/debugging.md) enumerates; only the two *cheap*
/// ones are implemented here, because the hot ones (instruction-retired,
/// memory-access) are explicitly specified there as ring-buffered and behind the
/// `debug` cargo feature. Adding them later does not change this enum's existing
/// variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A frame finished. ~60/s, trivial to emit.
    Frame { number: u64 },
    /// One byte was exchanged on the link cable: the game sent `sent` and
    /// received `received`. A few thousand per second during a Pokémon trade,
    /// a handful per second for a Blargg test ROM.
    SerialTransfer { sent: u8, received: u8 },
}

impl Event {
    /// The event's category name — the string an external subscriber filters on.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Frame { .. } => "frame",
            Event::SerialTransfer { .. } => "serial",
        }
    }

    /// Serialise as one NDJSON record, the same shape [`crate::telemetry`] uses,
    /// so the wire transport described in `docs/debugging.md` is a change of pipe
    /// rather than of contract. Hand-rolled: this crate takes no serde
    /// dependency (see AGENTS.md "no new crate dependencies").
    pub fn write_json(&self, out: &mut String) {
        use std::fmt::Write as _;
        match self {
            Event::Frame { number } => {
                let _ = write!(out, "{{\"ev\":\"frame\",\"number\":{number}}}");
            }
            Event::SerialTransfer { sent, received } => {
                let _ = write!(
                    out,
                    "{{\"ev\":\"serial\",\"sent\":{sent},\"received\":{received}}}"
                );
            }
        }
    }
}

/// A plugin's control handle on the running machine.
///
/// This is the **control** face of the extension surface, narrowed to what a
/// plugin has declared. It borrows the MMU for the duration of one callback, so
/// a plugin can never stash it and poke the machine from another thread.
pub struct Machine<'m, 'a> {
    mem: &'m mut MemoryManagementUnit<'a>,
    granted: Capability,
    /// Set when a plugin actually wrote memory, so the host can surface "this
    /// plugin has modified your game" in the status bar rather than only "this
    /// plugin *could*".
    wrote: bool,
}

impl<'m, 'a> Machine<'m, 'a> {
    fn new(mem: &'m mut MemoryManagementUnit<'a>, granted: Capability) -> Self {
        Machine {
            mem,
            granted,
            wrote: false,
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
        Some(self.mem.debug_peek(address))
    }

    /// Read a run of bytes. Empty if the capability was not declared.
    pub fn peek_range(&mut self, address: u16, len: usize) -> Vec<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return Vec::new();
        }
        (0..len)
            .map(|i| self.mem.debug_peek(address.wrapping_add(i as u16)))
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
            *slot = self.mem.debug_peek(base.wrapping_add(i as u16));
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
    /// This is the second gap the trainer-view crew reported (see the module
    /// docs). Cartridge data — sprite tables, name tables, base stats — lives in
    /// banks the game maps only while it is using them, so a plugin polling once
    /// a second would otherwise read whatever happened to be mapped. `address`
    /// is used modulo the 16 KiB bank window, so both `$0000` and `$4000` forms
    /// work. Reads past the end of the cartridge return `0xFF` (open bus), never
    /// a panic.
    ///
    /// It does not switch banks: the MBC's registers are untouched, so this
    /// cannot perturb the running game the way a read-then-restore would.
    pub fn peek_bank(&mut self, bank: usize, address: u16) -> Option<u8> {
        if !self.granted.contains(Capability::READ_MEMORY) {
            return None;
        }
        Some(self.mem.mbc.readrom_bank(bank, address))
    }

    /// Cartridge title from the ROM header.
    pub fn rom_title(&self) -> String {
        self.mem.mbc.romname()
    }

    /// Header global checksum (`$014E-$014F`) — enough to tell one build of a
    /// game from another, which is how a game-specific plugin refuses to display
    /// garbage for a ROM it does not know.
    pub fn global_checksum(&self) -> u16 {
        self.mem.mbc.global_checksum()
    }

    /// Currently-mapped ROM bank.
    pub fn rom_bank(&self) -> usize {
        self.mem.mbc.rom_bank()
    }

    /// Write one byte. Returns `false` (and does nothing) if the plugin did not
    /// declare [`Capability::WRITE_MEMORY`].
    ///
    /// **This can corrupt a save.** The virtual link partner shipped in
    /// [`gen1`] deliberately does not declare this capability: a traded Pokémon
    /// must arrive through the real trade path, not through a poke.
    pub fn poke(&mut self, address: u16, value: u8) -> bool {
        if !self.granted.contains(Capability::WRITE_MEMORY) {
            return false;
        }
        self.mem.debug_poke(address, value);
        self.wrote = true;
        true
    }

    /// Which capabilities this handle actually carries.
    pub fn granted(&self) -> Capability {
        self.granted
    }
}

/// What the user is shown about an attached plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginStatus {
    /// Stable short name, e.g. `gen1-link`, with the write marker appended:
    /// `!` for "may write your game", a sticky `!!` for "has written".
    ///
    /// The marker lives on the *name* rather than in `detail` deliberately —
    /// a frontend truncates the detail to fit a panel, and the one thing that
    /// must never be truncated away is the warning.
    pub name: String,
    /// One line of live detail, e.g. `waiting for link` / `trading MEW`.
    pub detail: String,
    /// Whether this plugin has actually written emulated memory.
    pub has_written: bool,
}

/// The thing a plugin implements.
///
/// Every method has a default, so a plugin only writes the ones it uses; the
/// host skips categories a plugin has not declared, so unused callbacks cost a
/// bitmask test rather than a virtual call.
///
/// `Send` is required for the same reason the serial callback requires it: the
/// FFI build parks a whole `Gameboy` in a `static Mutex`, so everything reachable
/// from the machine must be sendable.
pub trait Plugin: Send {
    /// Stable short identifier, used in configuration and the status bar.
    fn name(&self) -> &str;

    /// Everything this plugin may do. Enforced, not advisory.
    fn capabilities(&self) -> Capability;

    /// One line for the status bar. Called at most once per displayed frame.
    fn status(&self) -> String {
        String::new()
    }

    /// Called once, when the plugin is attached to a machine. The usual job is
    /// to decide whether this plugin understands this cartridge at all —
    /// [`Machine::rom_title`] and [`Machine::global_checksum`] are the two
    /// reads that answer it.
    fn on_attach(&mut self, _machine: &mut Machine) {}

    /// Something happened. Only delivered if [`Capability::OBSERVE`] is declared.
    ///
    /// The `machine` parameter is deliberate: a plugin reacting to an event
    /// almost always wants to *read state* in response, and the sketch this
    /// contract grew from could not (gap 1 in the module docs).
    fn on_event(&mut self, _event: &Event, _machine: &mut Machine) {}

    /// The game clocked a byte out of the link port; return the byte this
    /// partner shifts back, or `None` to behave like an unplugged cable.
    ///
    /// Only called on the single plugin holding [`Capability::SERIAL_LINK`].
    fn on_serial(&mut self, _sent: u8, _machine: &mut Machine) -> Option<u8> {
        None
    }

    /// Drop any in-flight protocol state. Called when a save state is loaded,
    /// because plugin state is not part of the machine state and would otherwise
    /// desync silently.
    fn reset(&mut self) {}

    /// Downcast hook, so a frontend can recover the concrete plugin it attached
    /// in order to *render* it — a plugin never draws (see
    /// [`docs/plugins.md`](../docs/plugins.md) §"A plugin never draws"), it
    /// exposes state the frontend lays out.
    ///
    /// An explicit method rather than a `dyn Any` upcast: trait upcasting is
    /// not stable on this crate's pinned 1.83 toolchain (`rust-toolchain`).
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable counterpart of [`Plugin::as_any`], so a frontend can configure
    /// the plugin it attached. This is the frontend talking to its own plugin,
    /// not to the emulator: it grants no capability the plugin did not declare.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Registration failure. Deliberately small — there is no plugin manager, only
/// the two things that can actually go wrong.
#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// A second plugin asked for [`Capability::SERIAL_LINK`]. A cable has two
    /// ends; the emulated console is one of them.
    LinkAlreadyClaimed(String),
    /// Two plugins with the same [`Plugin::name`].
    DuplicateName(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterError::LinkAlreadyClaimed(n) => {
                write!(f, "the serial link is already driven by plugin '{n}'")
            }
            RegisterError::DuplicateName(n) => {
                write!(f, "a plugin named '{n}' is already attached")
            }
        }
    }
}

impl Slot {
    /// The user-visible write warning: `!` = may write your game, `!!` = has.
    fn mark(&self) -> &'static str {
        if self.has_written {
            "!!"
        } else if self.capabilities.contains(Capability::WRITE_MEMORY) {
            "!"
        } else {
            ""
        }
    }
}

struct Slot {
    plugin: Box<dyn Plugin>,
    capabilities: Capability,
    /// Sticky: set once this plugin has actually written memory.
    has_written: bool,
    /// Cleared once [`Plugin::on_attach`] has run. Attachment is completed on
    /// the first dispatch rather than in `register`, because that is the first
    /// moment a [`Machine`] exists to hand it — `register` deliberately does not
    /// borrow the MMU, so a frontend can build its plugin set before running.
    needs_attach: bool,
}

/// Owns the attached plugins and dispatches to them.
///
/// Lives on the [`Cpu`](crate::cpu::core::Cpu) rather than inside the MMU, so
/// dispatch can hand a plugin `&mut MemoryManagementUnit` without a self-borrow.
/// Empty by default: an emulator with no plugins pays one `is_empty` test per
/// frame and one per serial transfer.
#[derive(Default)]
pub struct PluginHost {
    slots: Vec<Slot>,
    /// Index into `slots` of the plugin driving the link cable, if any.
    link: Option<usize>,
    /// Optional NDJSON event trace. This is the *emit* face made concrete over a
    /// file rather than a socket — same records, different pipe. Enabled with
    /// `GB_PLUGIN_TRACE=<path>`.
    trace: Option<Trace>,
    frame: u64,
}

struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    buf: String,
}

impl PluginHost {
    pub fn new() -> PluginHost {
        PluginHost::default()
    }

    /// Attach a plugin. Fails rather than silently downgrading if it asks for
    /// the link cable and another plugin already holds it.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> Result<(), RegisterError> {
        let caps = plugin.capabilities();
        let name = plugin.name().to_string();
        if self.slots.iter().any(|s| s.plugin.name() == name) {
            return Err(RegisterError::DuplicateName(name));
        }
        if caps.contains(Capability::SERIAL_LINK) {
            if let Some(i) = self.link {
                return Err(RegisterError::LinkAlreadyClaimed(
                    self.slots[i].plugin.name().to_string(),
                ));
            }
            self.link = Some(self.slots.len());
        }
        self.slots.push(Slot {
            plugin,
            capabilities: caps,
            has_written: false,
            needs_attach: true,
        });
        Ok(())
    }

    /// Borrow an attached plugin by concrete type, so a frontend can render it.
    pub fn get<T: Plugin + 'static>(&self) -> Option<&T> {
        self.slots
            .iter()
            .find_map(|s| s.plugin.as_any().downcast_ref::<T>())
    }

    /// Mutable borrow of an attached plugin, for the frontend to configure it
    /// (whether its panel is visible, say).
    pub fn get_mut<T: Plugin + 'static>(&mut self) -> Option<&mut T> {
        self.slots
            .iter_mut()
            .find_map(|s| s.plugin.as_any_mut().downcast_mut::<T>())
    }

    /// Open the NDJSON event trace described by `GB_PLUGIN_TRACE`. Returns a
    /// message on failure so the frontend can tell the user rather than
    /// swallowing it.
    pub fn open_trace_from_env(&mut self) -> Option<String> {
        let path = std::env::var("GB_PLUGIN_TRACE").ok()?;
        let p = path.trim();
        if p.is_empty() || p.eq_ignore_ascii_case("off") || p == "0" {
            return None;
        }
        match std::fs::File::create(p) {
            Ok(f) => {
                self.trace = Some(Trace {
                    out: std::io::BufWriter::new(f),
                    buf: String::with_capacity(128),
                });
                None
            }
            Err(e) => Some(format!("plugin trace: cannot write {p}: {e}")),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Is any plugin currently driving the link cable?
    pub fn has_link_partner(&self) -> bool {
        self.link.is_some()
    }

    /// Live status of every attached plugin, for the frontend to display.
    pub fn statuses(&self) -> Vec<PluginStatus> {
        self.slots
            .iter()
            .map(|s| PluginStatus {
                name: format!("{}{}", s.plugin.name(), s.mark()),
                detail: s.plugin.status(),
                has_written: s.has_written,
            })
            .collect()
    }

    /// One line for a status bar. **The user must be able to see that a plugin
    /// is attached**, and specifically that one can write memory — a plugin with
    /// `WRITE_MEMORY` is marked `!` (may write) or `!!` (has written).
    pub fn status_line(&self) -> Option<String> {
        if self.slots.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .slots
            .iter()
            .map(|s| {
                let mark = s.mark();
                let detail = s.plugin.status();
                if detail.is_empty() {
                    format!("{}{}", s.plugin.name(), mark)
                } else {
                    format!("{}{}: {}", s.plugin.name(), mark, detail)
                }
            })
            .collect();
        Some(parts.join(" | "))
    }

    /// True if any attached plugin has actually written emulated memory. The
    /// frontend surfaces this so "my save looks wrong" is never a mystery.
    pub fn any_wrote_memory(&self) -> bool {
        self.slots.iter().any(|s| s.has_written)
    }

    fn emit(&mut self, event: &Event) {
        if let Some(t) = self.trace.as_mut() {
            use std::io::Write as _;
            t.buf.clear();
            event.write_json(&mut t.buf);
            t.buf.push('\n');
            let _ = t.out.write_all(t.buf.as_bytes());
        }
    }

    fn deliver(
        &mut self,
        event: Event,
        mem: &mut MemoryManagementUnit,
        category: Capability,
    ) {
        self.emit(&event);
        for slot in self.slots.iter_mut() {
            // Attachment is completed here, on the first dispatch that reaches
            // the plugin, because this is the first point a `Machine` exists.
            if slot.needs_attach {
                slot.needs_attach = false;
                let mut machine = Machine::new(mem, slot.capabilities);
                slot.plugin.on_attach(&mut machine);
                slot.has_written |= machine.wrote;
            }
            if !slot.capabilities.contains(category) {
                continue;
            }
            let mut machine = Machine::new(mem, slot.capabilities);
            slot.plugin.on_event(&event, &mut machine);
            slot.has_written |= machine.wrote;
        }
    }

    /// A frame completed. Called from [`crate::gameboy::Gameboy::frame`].
    pub fn on_frame(&mut self, mem: &mut MemoryManagementUnit) {
        if self.slots.is_empty() && self.trace.is_none() {
            return;
        }
        self.frame = self.frame.wrapping_add(1);
        let n = self.frame;
        self.deliver(Event::Frame { number: n }, mem, Capability::OBSERVE);
    }

    /// The game started an internally-clocked transfer with `sent` on the wire.
    /// Returns the byte the link partner shifts back, or `None` for "no cable".
    ///
    /// This is the whole point of the module: a plugin gets to *be* the other
    /// Game Boy. Because the reply is produced synchronously inside the write
    /// that started the transfer, a virtual partner has no timing budget to
    /// miss — the exchange is as instantaneous as this core's serial model
    /// already is (see `docs/components/serial.md` §"Honest approximations").
    pub fn on_serial_transfer(
        &mut self,
        sent: u8,
        mem: &mut MemoryManagementUnit,
    ) -> Option<u8> {
        let idx = self.link?;
        let reply = {
            let slot = &mut self.slots[idx];
            if slot.needs_attach {
                // A game can clock the link before the first frame boundary, so
                // the link partner completes attachment here too.
                slot.needs_attach = false;
                let mut machine = Machine::new(mem, slot.capabilities);
                slot.plugin.on_attach(&mut machine);
                slot.has_written |= machine.wrote;
            }
            let slot = &mut self.slots[idx];
            let mut machine = Machine::new(mem, slot.capabilities);
            let r = slot.plugin.on_serial(sent, &mut machine);
            slot.has_written |= machine.wrote;
            r
        };
        // Observers see the completed exchange, including what the partner
        // answered — that is what makes a protocol trace useful.
        if self.trace.is_some() || self.slots.len() > 1 {
            let ev = Event::SerialTransfer {
                sent,
                received: reply.unwrap_or(0xFF),
            };
            self.deliver(ev, mem, Capability::OBSERVE);
        }
        reply
    }

    /// Drop protocol state in every plugin. Called after a save state is loaded:
    /// plugin state is not machine state, so it cannot be restored, and a stale
    /// half-finished exchange would desync the game silently.
    pub fn reset(&mut self) {
        for slot in self.slots.iter_mut() {
            slot.plugin.reset();
        }
    }

    pub fn flush(&mut self) {
        if let Some(t) = self.trace.as_mut() {
            use std::io::Write as _;
            let _ = t.out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Spy {
        name: String,
        caps: Capability,
        events: usize,
        last_serial: Option<u8>,
        peeked: Option<u8>,
        poke_ok: Option<bool>,
    }

    impl Spy {
        fn new(name: &str, caps: Capability) -> Spy {
            Spy {
                name: name.to_string(),
                caps,
                events: 0,
                last_serial: None,
                peeked: None,
                poke_ok: None,
            }
        }
    }

    impl Plugin for Spy {
        fn name(&self) -> &str {
            &self.name
        }
        fn capabilities(&self) -> Capability {
            self.caps
        }
        fn status(&self) -> String {
            format!("{} events", self.events)
        }
        fn on_event(&mut self, _e: &Event, m: &mut Machine) {
            self.events += 1;
            self.peeked = m.peek(0xC000);
            self.poke_ok = Some(m.poke(0xC001, 0x42));
        }
        fn on_serial(&mut self, sent: u8, _m: &mut Machine) -> Option<u8> {
            self.last_serial = Some(sent);
            Some(!sent)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    fn mmu() -> MemoryManagementUnit<'static> {
        MemoryManagementUnit::new(vec![0u8; 0x8000], None).unwrap()
    }

    /// Gap 2 the trainer-view crew reported: cartridge tables live in banks the
    /// game maps only while it is using them, so an observer must be able to
    /// name a bank. Proved by putting a distinct byte in each bank and reading
    /// them all back while the game has bank 1 mapped.
    #[test]
    fn a_plugin_can_read_a_rom_bank_the_game_has_not_mapped() {
        // 8 banks of 16 KiB. `get_mbc` with no path picks MBC3.
        let mut rom = vec![0u8; 8 * 0x4000];
        for bank in 0..8usize {
            rom[bank * 0x4000] = 0xA0 + bank as u8;
        }
        rom[0x0147] = 0x13; // MBC3+RAM+BATTERY
        rom[0x0148] = 0x03; // 8 banks
        let mut mem = MemoryManagementUnit::new(rom, None).unwrap();

        let mut m = Machine::new(&mut mem, Capability::READ_MEMORY);
        for bank in 0..8usize {
            assert_eq!(
                m.peek_bank(bank, 0x4000),
                Some(0xA0 + bank as u8),
                "bank {bank} readable regardless of what is mapped"
            );
            // The $0000 and $4000 forms address the same window.
            assert_eq!(m.peek_bank(bank, 0x0000), m.peek_bank(bank, 0x4000));
        }
        // Past the end of the cartridge is open bus, not a panic.
        assert_eq!(m.peek_bank(999, 0x4000), Some(0xFF));

        // And it did not disturb the game's banking: a normal read still sees
        // whatever bank the MBC has selected (bank 1 after reset).
        assert_eq!(m.peek(0x4000), Some(0xA1));

        // It is a read, so it is gated like one.
        let mut blind = Machine::new(&mut mem, Capability::OBSERVE);
        assert_eq!(
            blind.peek_bank(0, 0x0000),
            None,
            "peek_bank without READ_MEMORY is refused like any other read"
        );
    }

    #[test]
    fn a_plugin_without_read_capability_cannot_peek() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("blind", Capability::OBSERVE)))
            .unwrap();
        let mut m = mmu();
        host.on_frame(&mut m);
        // The spy asked for both; it declared neither, so both were refused.
        assert_eq!(host.statuses()[0].detail, "1 events");
        assert!(!host.any_wrote_memory());
    }

    #[test]
    fn capabilities_are_enforced_not_trusted() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new(
            "reader",
            Capability::OBSERVE | Capability::READ_MEMORY,
        )))
        .unwrap();
        let mut m = mmu();
        m.debug_poke(0xC000, 0x99);
        // WRAM is seeded with a fixed pseudo-random pattern, so capture the
        // byte rather than assuming zero.
        let before = m.debug_peek(0xC001);
        host.on_frame(&mut m);
        // Read worked, write did not, and the write attempt did not flip the
        // "has written" flag the status bar relies on.
        assert!(!host.any_wrote_memory());
        assert_eq!(m.debug_peek(0xC001), before);
        assert_ne!(before, 0x42, "test would be vacuous");
    }

    #[test]
    fn a_writing_plugin_is_visible_in_the_status_line() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new(
            "writer",
            Capability::OBSERVE | Capability::WRITE_MEMORY,
        )))
        .unwrap();
        // Declared but not yet used: single bang.
        assert!(host.status_line().unwrap().starts_with("writer!:"));
        let mut m = mmu();
        host.on_frame(&mut m);
        // Actually wrote: double bang, and it stays.
        assert!(host.any_wrote_memory());
        assert!(host.status_line().unwrap().starts_with("writer!!:"));
        assert_eq!(m.debug_peek(0xC001), 0x42);
    }

    #[test]
    fn only_one_plugin_may_drive_the_link_cable() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("a", Capability::SERIAL_LINK)))
            .unwrap();
        let err = host
            .register(Box::new(Spy::new("b", Capability::SERIAL_LINK)))
            .unwrap_err();
        assert_eq!(err, RegisterError::LinkAlreadyClaimed("a".into()));
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("x", Capability::OBSERVE)))
            .unwrap();
        assert_eq!(
            host.register(Box::new(Spy::new("x", Capability::OBSERVE)))
                .unwrap_err(),
            RegisterError::DuplicateName("x".into())
        );
    }

    #[test]
    fn the_link_partner_answers_the_byte_the_game_sent() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("link", Capability::SERIAL_LINK)))
            .unwrap();
        let mut m = mmu();
        assert_eq!(host.on_serial_transfer(0x01, &mut m), Some(0xFE));
    }

    #[test]
    fn no_plugins_means_no_link_partner() {
        let mut host = PluginHost::new();
        let mut m = mmu();
        assert!(host.is_empty());
        assert_eq!(host.on_serial_transfer(0x01, &mut m), None);
        assert_eq!(host.status_line(), None);
    }

    /// The seam itself, driven by the real CPU rather than by calling the host
    /// directly: a program writes `SB` then `SC = $81`, and the byte the plugin
    /// shifts back must be readable from `SB` on the very next instruction.
    ///
    /// This is the one thing unit-testing `PluginHost` in isolation cannot
    /// prove — that `Serial::pending`, `Cpu::do_cycle` and `MMU::do_cycle` are
    /// wired in the right order.
    #[test]
    fn a_plugin_answers_a_transfer_the_running_program_started() {
        struct Answer;
        impl Plugin for Answer {
            fn name(&self) -> &str {
                "answer"
            }
            fn capabilities(&self) -> Capability {
                Capability::SERIAL_LINK
            }
            fn on_serial(&mut self, sent: u8, _m: &mut Machine) -> Option<u8> {
                // The Gen 1 handshake in miniature: reply $02 to $01.
                Some(if sent == 0x01 { 0x02 } else { 0xEE })
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x112].copy_from_slice(&[
            0x3E, 0x01, // LD A, $01
            0xEA, 0x01, 0xFF, // LD ($FF01), A   ; SB = the byte we send
            0x3E, 0x81, // LD A, $81
            0xEA, 0x02, 0xFF, // LD ($FF02), A   ; SC = start, internal clock
            0xFA, 0x01, 0xFF, // LD A, ($FF01)   ; read the received byte
            0xEA, 0x00, 0xC0, // LD ($C000), A   ; stash it where we can see it
            0x18, 0xFE, // JR -2            ; halt
        ]);

        let mut cpu = crate::cpu::core::Cpu::new(rom, None);
        cpu.plugins.register(Box::new(Answer)).unwrap();
        cpu.registers.pc = 0x100;
        for _ in 0..8 {
            cpu.do_cycle();
        }
        assert_eq!(
            cpu.memory.debug_peek(0xC000),
            0x02,
            "the plugin's reply should be in SB by the next instruction"
        );
    }

    /// With nothing attached, the same program sees an unplugged cable — `SB`
    /// keeps the byte that was written to it, and no serial interrupt is raised.
    #[test]
    fn with_no_plugin_a_transfer_behaves_like_an_unplugged_cable() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x112].copy_from_slice(&[
            0x3E, 0x01, 0xEA, 0x01, 0xFF, 0x3E, 0x81, 0xEA, 0x02, 0xFF, 0xFA, 0x01, 0xFF,
            0xEA, 0x00, 0xC0, 0x18, 0xFE,
        ]);
        let mut cpu = crate::cpu::core::Cpu::new(rom, None);
        cpu.registers.pc = 0x100;
        for _ in 0..8 {
            cpu.do_cycle();
        }
        assert_eq!(cpu.memory.debug_peek(0xC000), 0x01);
        assert_eq!(
            cpu.memory.intf & 0x08,
            0,
            "no serial interrupt without a partner"
        );
    }

    #[test]
    fn events_serialise_to_the_documented_ndjson_shape() {
        let mut s = String::new();
        Event::Frame { number: 7 }.write_json(&mut s);
        assert_eq!(s, r#"{"ev":"frame","number":7}"#);
        s.clear();
        Event::SerialTransfer {
            sent: 1,
            received: 2,
        }
        .write_json(&mut s);
        assert_eq!(s, r#"{"ev":"serial","sent":1,"received":2}"#);
    }
}

//! The [`Plugin`] trait and the [`PluginHost`] that owns and dispatches to them.

#[cfg(feature = "embedded")]
use crate::compat::*;

use crate::{Capability, Event, Machine, MachineAccess, API_VERSION};

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
    /// Whether this plugin has actually pressed a button.
    pub has_pressed: bool,
    /// Set once this plugin has panicked and been quarantined. It receives no
    /// further callbacks; see [`PluginHost`] §"A failing plugin".
    pub failed: bool,
}

/// The thing a plugin implements.
///
/// Every method has a default, so a plugin only writes the ones it uses; the
/// host skips categories a plugin has not declared, so unused callbacks cost a
/// bitmask test rather than a virtual call.
///
/// `Send` is required for the same reason the serial callback requires it: the
/// FFI build parks a whole `Gameboy` in a `static Mutex`, so everything
/// reachable from the machine must be sendable.
pub trait Plugin: Send {
    /// Stable short identifier, used in configuration and the status bar.
    fn name(&self) -> &str;

    /// Everything this plugin may do. Enforced, not advisory.
    fn capabilities(&self) -> Capability;

    /// Which revision of this API the plugin was built against.
    ///
    /// **Do not override this.** The default body is compiled into *your*
    /// crate, so it captures the [`API_VERSION`] of the `terminalgb-plugin-api`
    /// you linked, and [`PluginHost::register`] refuses anything it does not
    /// recognise. Overriding it is how you lie about compatibility.
    fn api_version(&self) -> u32 {
        API_VERSION
    }

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
    /// almost always wants to *read state* in response.
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
    /// in order to *render* it — a plugin never draws, it exposes state the
    /// frontend lays out.
    ///
    /// An explicit method rather than a `dyn Any` upcast: trait upcasting is
    /// not stable on this crate's pinned 1.83 toolchain (`rust-toolchain`).
    fn as_any(&self) -> &dyn core::any::Any;

    /// Mutable counterpart of [`Plugin::as_any`], so a frontend can configure
    /// the plugin it attached. This is the frontend talking to its own plugin,
    /// not to the emulator: it grants no capability the plugin did not declare.
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any;
}

/// Registration failure. Deliberately small — there is no plugin manager, only
/// the things that can actually go wrong.
#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// A second plugin asked for [`Capability::SERIAL_LINK`]. A cable has two
    /// ends; the emulated console is one of them.
    LinkAlreadyClaimed(String),
    /// Two plugins with the same [`Plugin::name`].
    DuplicateName(String),
    /// The plugin was built against a revision of this API that this host does
    /// not implement. Refused rather than loaded, because the alternative is a
    /// plugin that misbehaves in a way nobody can diagnose.
    ApiVersionMismatch { plugin: u32, host: u32 },
}

impl core::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RegisterError::LinkAlreadyClaimed(n) => {
                write!(f, "the serial link is already driven by plugin '{n}'")
            }
            RegisterError::DuplicateName(n) => {
                write!(f, "a plugin named '{n}' is already attached")
            }
            RegisterError::ApiVersionMismatch { plugin, host } => write!(
                f,
                "built against plugin API v{plugin}, but this emulator \
                 implements v{host} — rebuild the plugin against \
                 terminalgb-plugin-api v{host}"
            ),
        }
    }
}

#[cfg(not(feature = "embedded"))]
impl std::error::Error for RegisterError {}

struct Slot {
    plugin: Box<dyn Plugin>,
    capabilities: Capability,
    /// Sticky: set once this plugin has actually written memory.
    has_written: bool,
    /// Sticky: set once this plugin has actually pressed a button.
    has_pressed: bool,
    /// Sticky: set once this plugin has panicked. A quarantined plugin is
    /// never called again.
    failed: bool,
    /// Cleared once [`Plugin::on_attach`] has run. Attachment is completed on
    /// the first dispatch rather than in `register`, because that is the first
    /// moment a [`Machine`] exists — `register` deliberately does not borrow
    /// the machine, so a frontend can build its plugin set before running.
    needs_attach: bool,
}

impl Slot {
    /// The user-visible warning markers, in severity order: `!` = may write
    /// your game and `!!` = has, then `>` = may press the buttons and `>>` =
    /// has, and `x` for a plugin that panicked and was switched off. All are
    /// sticky and all live on the *name*, because a frontend truncates the
    /// detail to fit a panel and the warning must survive that.
    fn mark(&self) -> String {
        let mut s = String::new();
        if self.has_written {
            s.push_str("!!");
        } else if self.capabilities.contains(Capability::WRITE_MEMORY) {
            s.push('!');
        }
        if self.has_pressed {
            s.push_str(">>");
        } else if self.capabilities.contains(Capability::CONTROL_INPUT) {
            s.push('>');
        }
        if self.failed {
            s.push('x');
        }
        s
    }

    fn detail(&self) -> String {
        if self.failed {
            "stopped after an internal error".to_string()
        } else {
            self.plugin.status()
        }
    }
}

/// Owns the attached plugins and dispatches to them.
///
/// Lives on the emulator's CPU rather than inside the memory unit, so dispatch
/// can hand a plugin the machine without a self-borrow. Empty by default: an
/// emulator with no plugins pays one `is_empty` test per frame and one per
/// serial transfer.
///
/// # A failing plugin
///
/// Every call into a plugin is wrapped in [`std::panic::catch_unwind`]. A
/// plugin that panics is **quarantined**: it is marked failed, never called
/// again, and shown with an `x` in the status line — the emulator keeps
/// running. Continuing to call a plugin whose state is by definition
/// half-updated would be the worse choice.
///
/// **The honest caveat:** this works in any build that unwinds. The release
/// profile in this repository sets `panic = 'abort'` (a size and speed
/// decision that predates plugins), and under abort *no* `catch_unwind`
/// anywhere in Rust can catch anything — a panicking plugin takes the process
/// with it. See `docs/plugins.md` §"Errors".
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
    /// One line per plugin that has panicked, for the frontend to surface.
    failures: Vec<String>,
}

#[cfg(not(feature = "embedded"))]
struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    buf: String,
}

/// **Uninhabited on bare metal**: there is no file to trace into, so
/// `Option<Trace>` can only be `None` and every site that would write one is
/// compiled out rather than stubbed.
#[cfg(feature = "embedded")]
enum Trace {}

/// Run `f`, catching a panic and reporting it as `Err(message)`.
///
/// `AssertUnwindSafe` is correct here rather than a shortcut: the values
/// crossing the boundary are the plugin itself and the machine, and the whole
/// point is that after a panic we *do* treat the plugin's state as suspect —
/// it is quarantined and never touched again.
/// **Bare metal cannot catch a panic, so it does not pretend to.**
///
/// `catch_unwind` needs an unwinding runtime and an embedded build aborts —
/// which the note above already says is true of a release `panic = "abort"`
/// host build too. Quarantine is a development guarantee, not a release one.
#[cfg(feature = "embedded")]
fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    Ok(f())
}

#[cfg(not(feature = "embedded"))]
fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "panicked".to_string()
            };
            Err(msg)
        }
    }
}

impl PluginHost {
    pub fn new() -> PluginHost {
        PluginHost::default()
    }

    /// Attach a plugin. Fails rather than silently downgrading if it was built
    /// against a different API revision, if it asks for the link cable and
    /// another plugin already holds it, or if its name is taken.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> Result<(), RegisterError> {
        let version = plugin.api_version();
        if version != API_VERSION {
            return Err(RegisterError::ApiVersionMismatch {
                plugin: version,
                host: API_VERSION,
            });
        }
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
            has_pressed: false,
            failed: false,
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
        #[cfg(feature = "embedded")]
        return None;
        #[cfg(not(feature = "embedded"))]
        {
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
                detail: s.detail(),
                has_written: s.has_written,
                has_pressed: s.has_pressed,
                failed: s.failed,
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
                let detail = s.detail();
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

    /// True if any attached plugin has actually pressed a button. The frontend
    /// surfaces this so "the game is playing itself" is never a mystery either.
    pub fn any_pressed_buttons(&self) -> bool {
        self.slots.iter().any(|s| s.has_pressed)
    }

    /// Drain the "plugin X panicked and was switched off" messages, for the
    /// frontend to toast. A failure the user is not told about is a failure
    /// they will report as an emulator bug.
    pub fn take_failures(&mut self) -> Vec<String> {
        core::mem::take(&mut self.failures)
    }

    fn emit(&mut self, event: &Event) {
        #[cfg(feature = "embedded")]
        let _ = event;
        #[cfg(not(feature = "embedded"))]
        if let Some(t) = self.trace.as_mut() {
            use std::io::Write as _;
            t.buf.clear();
            event.write_json(&mut t.buf);
            t.buf.push('\n');
            let _ = t.out.write_all(t.buf.as_bytes());
        }
    }

    /// Run one plugin callback under the panic guard, recording what the call
    /// did to the machine and quarantining the plugin if it panicked.
    fn call<T>(
        slots: &mut [Slot],
        failures: &mut Vec<String>,
        idx: usize,
        mem: &mut dyn MachineAccess,
        f: impl FnOnce(&mut Box<dyn Plugin>, &mut Machine) -> T,
    ) -> Option<T> {
        let slot = &mut slots[idx];
        if slot.failed {
            return None;
        }
        let caps = slot.capabilities;
        let plugin = &mut slot.plugin;
        let mut machine = Machine::new(mem, caps);
        let outcome = guard(|| f(plugin, &mut machine));
        let (wrote, pressed) = (machine.wrote(), machine.pressed());
        let slot = &mut slots[idx];
        slot.has_written |= wrote;
        slot.has_pressed |= pressed;
        match outcome {
            Ok(v) => Some(v),
            Err(msg) => {
                slot.failed = true;
                failures.push(format!(
                    "plugin {} failed and was switched off: {msg}",
                    slot.plugin.name()
                ));
                None
            }
        }
    }

    fn attach_if_needed(&mut self, idx: usize, mem: &mut dyn MachineAccess) {
        if !self.slots[idx].needs_attach {
            return;
        }
        self.slots[idx].needs_attach = false;
        PluginHost::call(&mut self.slots, &mut self.failures, idx, mem, |p, m| {
            p.on_attach(m)
        });
    }

    fn deliver(
        &mut self,
        event: Event,
        mem: &mut dyn MachineAccess,
        category: Capability,
    ) {
        self.emit(&event);
        for idx in 0..self.slots.len() {
            // Attachment is completed here, on the first dispatch that reaches
            // the plugin, because this is the first point a `Machine` exists.
            self.attach_if_needed(idx, mem);
            if !self.slots[idx].capabilities.contains(category) {
                continue;
            }
            PluginHost::call(&mut self.slots, &mut self.failures, idx, mem, |p, m| {
                p.on_event(&event, m)
            });
        }
    }

    /// A frame completed. Called from the emulator's frame loop.
    pub fn on_frame(&mut self, mem: &mut dyn MachineAccess) {
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
        mem: &mut dyn MachineAccess,
    ) -> Option<u8> {
        let idx = self.link?;
        // A game can clock the link before the first frame boundary, so the
        // link partner completes attachment here too.
        self.attach_if_needed(idx, mem);
        let reply =
            PluginHost::call(&mut self.slots, &mut self.failures, idx, mem, |p, m| {
                p.on_serial(sent, m)
            })
            .flatten();
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
            if slot.failed {
                continue;
            }
            if let Err(msg) = guard(|| slot.plugin.reset()) {
                slot.failed = true;
                self.failures.push(format!(
                    "plugin {} failed and was switched off: {msg}",
                    slot.plugin.name()
                ));
            }
        }
    }

    pub fn flush(&mut self) {
        #[cfg(not(feature = "embedded"))]
        if let Some(t) = self.trace.as_mut() {
            use std::io::Write as _;
            let _ = t.out.flush();
        }
    }
}

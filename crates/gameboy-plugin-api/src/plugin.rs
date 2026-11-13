//! [`Plugin`] — the trait you implement — and what the user is shown about it.

use crate::capability::Capability;
use crate::event::Event;
use crate::machine::Machine;
use crate::view::PluginView;
use crate::{ApiVersion, API_VERSION};

/// What the user is shown about an attached plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginStatus {
    /// Stable short name, e.g. `gen1-link`, with the warning markers appended:
    /// `!` for "may write your game", a sticky `!!` for "has written", `>` for
    /// "may press the buttons" and `>>` for "has pressed".
    ///
    /// The markers live on the *name* rather than in `detail` deliberately —
    /// a frontend truncates the detail to fit a panel, and the one thing that
    /// must never be truncated away is the warning.
    pub name: String,
    /// One line of live detail, e.g. `waiting for link` / `trading MEW`.
    pub detail: String,
    /// Whether this plugin has actually written emulated memory.
    pub has_written: bool,
    /// Whether this plugin has actually pressed a button.
    pub has_pressed: bool,
    /// Set once the plugin has been retired — because it called
    /// [`Machine::fail`], or because it panicked. It receives no further
    /// callbacks; the string says why.
    pub retired: Option<String>,
}

/// The thing a plugin implements.
///
/// Every method except [`Plugin::name`], [`Plugin::capabilities`] and the two
/// downcast hooks has a default, so a plugin only writes the ones it uses; the
/// host skips categories a plugin has not declared, so unused callbacks cost a
/// bitmask test rather than a virtual call.
///
/// `Send` is required because the emulator's C FFI parks a whole console in a
/// `static Mutex`, so everything reachable from the machine must be sendable.
///
/// # A minimal plugin
///
/// ```
/// use gameboy_plugin_api::{Capability, Event, Machine, Plugin};
///
/// struct FrameCounter(u64);
///
/// impl Plugin for FrameCounter {
///     fn name(&self) -> &str { "frame-counter" }
///     fn capabilities(&self) -> Capability { Capability::OBSERVE }
///     fn status(&self) -> String { format!("{} frames", self.0) }
///     fn on_event(&mut self, event: &Event, _m: &mut Machine) {
///         if let Event::Frame { number } = event { self.0 = *number; }
///     }
///     fn as_any(&self) -> &dyn std::any::Any { self }
///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
/// }
/// ```
pub trait Plugin: Send {
    /// Stable short identifier, used in configuration and the status bar. This
    /// is the name a user writes in `plugins = …`, so treat it as part of your
    /// public interface.
    fn name(&self) -> &str;

    /// Everything this plugin may do. Enforced, not advisory.
    fn capabilities(&self) -> Capability;

    /// Which version of this crate's contract the plugin was compiled against.
    ///
    /// **Do not override this.** The default reports the [`API_VERSION`] of the
    /// `gameboy-plugin-api` your crate compiled against, which is exactly the
    /// question the host is asking. Overriding it is how you claim
    /// compatibility you do not have.
    fn api_version(&self) -> ApiVersion {
        API_VERSION
    }

    /// One line for the status bar. Called at most once per displayed frame.
    fn status(&self) -> String {
        String::new()
    }

    /// A panel the frontend may show when the user asks for it. `None` (the
    /// default) means this plugin has nothing to show beyond its status line.
    ///
    /// See [`PluginView`] for why a plugin publishes a view rather than
    /// drawing.
    fn view(&self) -> Option<PluginView> {
        None
    }

    /// Called once, when the plugin is attached to a machine. The usual job is
    /// to decide whether this plugin understands this cartridge at all —
    /// [`Machine::rom_title`] and [`Machine::global_checksum`] are the two
    /// reads that answer it.
    fn on_attach(&mut self, _machine: &mut Machine) {}

    /// Something happened. Only delivered if [`Capability::OBSERVE`] is
    /// declared.
    ///
    /// The `machine` parameter is deliberate: a plugin reacting to an event
    /// almost always wants to *read state* in response.
    ///
    /// [`Event`] is `#[non_exhaustive]` — match with a `_ =>` arm so a new
    /// event category does not break your build.
    fn on_event(&mut self, _event: &Event, _machine: &mut Machine) {}

    /// The game clocked a byte out of the link port; return the byte this
    /// partner shifts back, or `None` to behave like an unplugged cable.
    ///
    /// Only called on the single plugin holding [`Capability::SERIAL_LINK`].
    fn on_serial(&mut self, _sent: u8, _machine: &mut Machine) -> Option<u8> {
        None
    }

    /// Drop any in-flight protocol state. Called when a save state is loaded,
    /// because plugin state is not part of the machine state and would
    /// otherwise desync silently.
    fn reset(&mut self) {}

    /// Downcast hook, so a frontend can recover the concrete plugin it attached
    /// in order to render it.
    ///
    /// An explicit method rather than a `dyn Any` upcast: trait upcasting is
    /// not stable on this project's pinned 1.83 toolchain.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable counterpart of [`Plugin::as_any`], so a frontend can configure
    /// the plugin it attached. This is the frontend talking to its own plugin,
    /// not to the emulator: it grants no capability the plugin did not declare.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

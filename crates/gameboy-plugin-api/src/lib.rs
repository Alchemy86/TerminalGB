//! **The public plugin interface for the [`gameboy`] emulator.**
//!
//! This crate is the whole contract between the emulator and a plugin. It has
//! **no dependencies** — not even on the emulator itself — so a plugin author
//! links one small crate, and an emulator release that changes the CPU cannot
//! break a plugin that never touched it.
//!
//! ```text
//!                     gameboy-plugin-api          <- you depend on this
//!                    /          |         \
//!            gameboy      plugins/trainer   your-plugin
//!           (the core)    plugins/gen1-link
//!                 \______ optional, feature-gated ______/
//! ```
//!
//! Start at [`Plugin`]. A five-line plugin is in
//! [`docs/plugins/writing-a-plugin.md`](https://github.com/Alchemy86/gameboy/blob/main/docs/plugins/writing-a-plugin.md),
//! and a complete working one is the `hello-plugin` crate in `plugins/example-hello/`.
//!
//! # What a plugin is
//!
//! **An in-process Rust object implementing [`Plugin`].** Not a dynamic
//! library, not a script engine, not a marketplace — a trait, in a crate that
//! links this one. That choice is the honest one for this project's
//! constraints: it needs no dependencies, it works identically on `wasm32`
//! (where there are no sockets and no `dlopen`), and it costs a null check per
//! frame when nothing is attached. Rust has no stable ABI, so a `dlopen`-style
//! loader would be unsound across compiler versions; that trade is written out
//! in the plugin guide rather than half-built here.
//!
//! The *messages* are designed for the wire even though the *transport* today
//! is a direct call: every [`Event`] serialises to one NDJSON record
//! ([`Event::write_json`]), and every reply a plugin can give is a small owned
//! value rather than a borrowed pointer. An external-process transport is
//! therefore an additive follow-up that does not reshape [`Plugin`].
//!
//! # What a plugin may do
//!
//! Exactly the things it **declares up front** via [`Plugin::capabilities`] —
//! see [`Capability`]. Declaration is *enforced*, not advisory: [`Machine`]
//! checks the calling plugin's declared set and refuses (returning `None` /
//! `false`) rather than trusting it. A plugin that never declares
//! [`Capability::WRITE_MEMORY`] **cannot** write, even if its code tries.
//!
//! # The trust model, stated
//!
//! **A plugin is trusted code that the user chose to compile in.** It is a
//! native Rust crate linked into the emulator binary; nothing here is a
//! sandbox, and this crate could not make one — a plugin could call
//! `std::process::exit` or spawn a thread and there is no boundary to stop it.
//!
//! What *is* enforced, and why it is still worth having:
//!
//! * **Capability scoping** ([`Capability`]) turns "this plugin only reads" from
//!   a promise in a README into a property the host checks on every call. It
//!   protects against a *mistake*, not against malice.
//! * **Visibility** ([`PluginStatus`], [`PluginHost::status_line`]) means a
//!   plugin that can write your save or press your buttons is marked on screen
//!   for as long as it is attached. A plugin cannot be active and invisible.
//! * **Fault isolation** ([`PluginHost`] quarantine): a plugin that panics is
//!   caught, retired, and reported — the emulator keeps running. See
//!   [`PluginHost::faults`].
//! * **Nothing loads implicitly.** [`PluginHost`] starts empty and a
//!   [`registry::PluginRegistry`] only produces what the user named.
//!
//! The honest summary: **capability-scoped, not sandboxed.** Install a plugin
//! the way you would install any Rust dependency — because that is what it is.
//!
//! # Compatibility
//!
//! [`API_VERSION`] is this crate's own version of the plugin contract, and
//! [`Plugin::api_version`] reports the one a plugin was compiled against.
//! [`PluginHost::register`] refuses an incompatible plugin with
//! [`RegisterError::ApiMismatch`] rather than letting it misbehave.
//!
//! Today the *compiler* is the real enforcer: two semver-incompatible copies of
//! this crate produce two unrelated `Plugin` traits, so a mismatch does not
//! link. The runtime check exists because it is the behaviour that has to be
//! defined *before* a transport (a shared library, a socket) makes the mismatch
//! possible at run time, and because "refuse clearly" is cheaper to build now
//! than to retrofit. See [`ApiVersion`].

pub mod capability;
pub mod event;
pub mod host;
pub mod input;
pub mod machine;
pub mod plugin;
pub mod registry;
pub mod view;

pub use capability::Capability;
pub use event::Event;
pub use host::{PluginHost, RegisterError};
pub use input::{KeypadKey, DEFAULT_GAP, DEFAULT_HOLD};
pub use machine::{Bus, Machine};
pub use plugin::{Plugin, PluginStatus};
pub use registry::{PluginConfig, PluginRegistry, PluginSpec};
pub use view::{PluginView, ViewColor, ViewLine, ViewStyle};

/// A version of the plugin contract: `major.minor`, independent of the
/// emulator's own version.
///
/// **Major** changes when something already in this crate changes meaning or
/// disappears — a renamed trait method, a removed capability, an [`Event`]
/// variant whose fields changed. **Minor** changes when something is added and
/// existing plugins keep working — a new [`Event`] variant, a new defaulted
/// [`Plugin`] method, a new [`Machine`] accessor.
///
/// So a plugin compiled against `1.2` runs on a host at `1.2` or `1.7`, and is
/// refused by a host at `1.1` (it may use something that host does not have) or
/// `2.0` (the meaning changed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub const fn new(major: u16, minor: u16) -> ApiVersion {
        ApiVersion { major, minor }
    }

    /// Can a plugin built against `self` run on a host offering `host`?
    pub const fn compatible_with(self, host: ApiVersion) -> bool {
        self.major == host.major && self.minor <= host.minor
    }
}

impl core::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The plugin contract this build of the crate implements. A plugin reports it
/// through the default [`Plugin::api_version`]; the host compares it against
/// its own copy of this constant.
pub const API_VERSION: ApiVersion = ApiVersion::new(1, 0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_is_same_major_and_no_newer_minor() {
        let host = ApiVersion::new(1, 4);
        assert!(ApiVersion::new(1, 4).compatible_with(host), "exact match");
        assert!(
            ApiVersion::new(1, 0).compatible_with(host),
            "an older plugin only uses what the host already had"
        );
        assert!(
            !ApiVersion::new(1, 5).compatible_with(host),
            "a newer plugin may use something this host does not have"
        );
        assert!(
            !ApiVersion::new(2, 0).compatible_with(host),
            "a major bump means the meaning changed"
        );
        assert!(
            !ApiVersion::new(0, 4).compatible_with(host),
            "and that is true downwards too"
        );
    }
}

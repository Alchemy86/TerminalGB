//! # The Game Boy emulator's plugin API
//!
//! Everything a plugin needs, and **nothing from the emulator's internals**.
//! This crate has no dependencies at all, so writing a plugin costs you one
//! `Cargo.toml` line and no build system archaeology.
//!
//! Start at [`Plugin`]. The 30-second version:
//!
//! ```
//! use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};
//!
//! struct FrameCounter { frames: u64 }
//!
//! impl Plugin for FrameCounter {
//!     fn name(&self) -> &str { "frame-counter" }
//!     fn capabilities(&self) -> Capability { Capability::OBSERVE }
//!     fn status(&self) -> String { format!("{} frames", self.frames) }
//!     fn on_event(&mut self, event: &Event, _m: &mut Machine) {
//!         if let Event::Frame { .. } = event { self.frames += 1; }
//!     }
//!     fn as_any(&self) -> &dyn std::any::Any { self }
//!     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
//! }
//! ```
//!
//! `plugins/example-tracer` is the same idea with tests and a README; copy that
//! directory to start. The full guide is
//! [`docs/plugins.md`](https://github.com/Alchemy86/TerminalGB/blob/main/docs/plugins.md).
//!
//! # What a plugin is
//!
//! **An in-process Rust object implementing [`Plugin`].** Not a dynamic
//! library, not a script engine, not a marketplace — a trait, implemented in a
//! crate that links this one. That choice is the honest one for this project's
//! constraints: it is dependency-free, it works identically on `wasm32` (where
//! there are no sockets and no `dlopen`), and it costs a null check per frame
//! when nothing is attached. Rust has no stable ABI, so a `dlopen`-style plugin
//! would have to be `extern "C"` end to end and would still break on a compiler
//! upgrade; `docs/plugins.md` §"Why not runtime loading" argues that out.
//!
//! The *messages* are designed for the wire even though the *transport* today
//! is a direct call: every [`Event`] serialises to the same NDJSON shape the
//! emulator's telemetry uses ([`Event::write_json`]), and every reply a plugin
//! can give is a small value, not a borrowed pointer. An external-process
//! transport is therefore an additive follow-up that does not reshape this
//! trait.
//!
//! # What a plugin may do
//!
//! Exactly these things, each of which it must **declare up front** via
//! [`Plugin::capabilities`]:
//!
//! * [`Capability::OBSERVE`] — receive [`Event`]s (frames, serial transfers),
//!   and look at the picture ([`Machine::frame_rgba`]).
//! * [`Capability::READ_MEMORY`] — [`Machine::peek`], [`Machine::peek_range`],
//!   [`Machine::peek_bank`].
//! * [`Capability::WRITE_MEMORY`] — [`Machine::poke`]. Corrupts saves if
//!   misused.
//! * [`Capability::SERIAL_LINK`] — be the console on the other end of the link
//!   cable ([`Plugin::on_serial`]).
//! * [`Capability::CONTROL_INPUT`] — drive the console's buttons, either as a
//!   raw latch ([`Machine::set_button`]) or as a press scheduled in frames
//!   ([`Machine::press`]).
//!
//! Declaration is enforced, not advisory: [`Machine`] checks the calling
//! plugin's declared capabilities and refuses (returning `None` / `false`)
//! rather than trusting it. A plugin that never declares `WRITE_MEMORY`
//! *cannot* write.
//!
//! # The trust model, stated
//!
//! **A plugin is trusted code that the user chose to compile in.** It is a
//! native Rust crate in the same process and address space as the emulator; it
//! could call `std::fs::remove_dir_all` or `std::process::exit` and nothing
//! here would stop it. Capabilities are **not a sandbox** — they are an
//! *interlock on the emulated machine*, so that a plugin's declared intent is
//! checked against what it actually does, and the user can see the difference
//! in the status bar. That distinction is the whole design:
//!
//! * What capabilities buy you: a read-only plugin provably cannot write the
//!   game, a plugin that starts pressing buttons is marked the moment it does,
//!   and "which plugin corrupted my save?" has an answer.
//! * What they do not buy you: protection from a hostile plugin. Install
//!   plugins the way you install any other Rust dependency — by reading them.
//!
//! Sandboxing would mean a wasm or subprocess boundary and a serialised
//! protocol on the hot path. That is a real option later (the [`Event`]
//! encoding is already wire-shaped for it) and a bad one now.
//!
//! # Safety and honesty rules the host keeps
//!
//! * **Nothing loads implicitly.** [`PluginHost`] starts empty. A plugin is
//!   registered by the frontend from explicit configuration (`plugins = …` in
//!   `config.toml`, or `GB_PLUGINS`), never by scanning a directory.
//! * **The user can always see it.** [`PluginHost::status_line`] is rendered in
//!   the terminal frontend's status panel whenever any plugin is attached, and
//!   plugins that hold `WRITE_MEMORY` or `CONTROL_INPUT` are flagged there.
//!   A plugin cannot be active and invisible.
//! * **One link partner.** A real cable has two ends, so at most one plugin may
//!   hold [`Capability::SERIAL_LINK`]; a second registration is rejected.
//! * **A panicking plugin is switched off, not fatal.** See [`PluginHost`]
//!   §"A failing plugin" — including the honest caveat about `panic = 'abort'`.
//! * **Save-state honesty.** Plugin state is *not* part of the machine state
//!   and is not serialised; loading a state mid-exchange resets attached
//!   plugins rather than silently desyncing.
//!
//! # Compatibility
//!
//! [`API_VERSION`] is the revision of this contract. A plugin declares it
//! implicitly through [`Plugin::api_version`], whose default body compiles the
//! constant *from the copy of this crate the plugin linked*, and
//! [`PluginHost::register`] refuses a mismatch with
//! [`RegisterError::ApiVersionMismatch`] rather than loading something that
//! will misbehave.
//!
//! Read that honestly: when plugins are linked at build time — the only
//! supported way today — Cargo has already made a mismatch impossible, because
//! two incompatible versions of this crate cannot satisfy one dependency graph.
//! The check is the contract's *statement* of how compatibility is declared,
//! it costs one `u32` comparison at registration, and it is the gate that
//! becomes load-bearing the day anything is loaded at runtime. It is not
//! pretending to catch a bug you can hit today.
//!
//! What actually protects a third-party plugin across our refactors is
//! ordinary semver on this crate, plus two deliberate choices: every
//! [`Plugin`] method except `name`, `capabilities` and the two `as_any` hooks
//! has a default body, and [`Event`] is `#[non_exhaustive]`. So we can add
//! events and callbacks without breaking you.

#![forbid(unsafe_code)]

mod capability;
mod event;
mod host;
mod input;
mod machine;

pub use capability::Capability;
pub use event::Event;
pub use host::{Plugin, PluginHost, PluginStatus, RegisterError};
pub use input::{KeypadKey, DEFAULT_GAP, DEFAULT_HOLD};
pub use machine::{Machine, MachineAccess};

/// The revision of the plugin contract this crate implements.
///
/// Bump it when a change would make an existing plugin *misbehave* rather than
/// fail to compile — a changed meaning for an existing method, a capability
/// that stops being enforced, a reordered `KeypadKey`. Adding an [`Event`]
/// variant or a defaulted [`Plugin`] method does **not** need a bump; those are
/// additive by construction. See the crate docs §"Compatibility".
pub const API_VERSION: u32 = 1;

#[cfg(test)]
mod tests;

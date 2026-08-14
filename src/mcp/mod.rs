//! **MCP server** — drive this emulator from any Model Context Protocol
//! client: Claude Desktop, Claude Code, or anything else that speaks MCP.
//!
//! Install steps, config blocks and the measured speed of each configuration:
//! [`docs/mcp.md`](../docs/mcp.md).
//!
//! # What this is, and what it is not
//!
//! It is a **thin wrapper over [`gameboy_plugin_agent`]**. That module already solved
//! the hard half — structured perception ([`gameboy_plugin_agent::observe`]) and
//! bounded, high-level action ([`gameboy_plugin_agent::action`]) — because an agent
//! needed them to play the game. Everything here is transport: JSON-RPC
//! framing, tool descriptions, and a host that owns the clock. No state
//! reading and no input handling is re-implemented; if you find yourself
//! adding some, it belongs in `agent`, not here.
//!
//! # Why a host binary and not a plugin
//!
//! A [`Plugin`](crate::plugin::Plugin) is handed a
//! [`Machine`](crate::plugin::Machine) *during a frame the emulator has already
//! decided to run*. That is the wrong shape for this in three ways, and each
//! one is a tool a client needs:
//!
//! 1. **It must drive the clock, not ride it.** `step`, `press_buttons` and
//!    `run_actions` decide how many frames run and when the call returns. A
//!    plugin cannot do that; it is told a frame happened.
//! 2. **Half the surface is above the machine.** `load_rom` replaces the
//!    cartridge, `reset` rebuilds it, `load_state` swaps the whole machine's
//!    contents. A `Machine` handle reaches memory and buttons — correctly so —
//!    and none of those three is expressible through it.
//! 3. **It is a frontend.** It sits where `screen::tui` sits: it owns host
//!    resources (stdio, a sound device, a video sink) and turns somebody's
//!    input into emulator operations.
//!
//! So it is a frontend that *uses* the plugin contract rather than joining it:
//! every read and every press goes through
//! [`Gameboy::with_machine`](crate::gameboy::Gameboy::with_machine) with an
//! explicit capability grant, which is how `write_memory` can be provably
//! absent unless the operator asked for it. Plugins can still be attached with
//! `--plugins`; the agent plugin deliberately cannot, because an MCP client
//! already *is* the agent.
//!
//! # Off unless asked for
//!
//! The whole module is behind the `mcp` cargo feature. Someone embedding this
//! core in their own tool or in hardware must not inherit an MCP stack, and the
//! feature adds **no third-party crate** — the JSON is
//! [`gameboy_plugin_agent::json`], the PNG and base64 are [`crate::termimage`].

pub mod config;
pub mod server;
pub mod session;

pub use config::{Options, Speed};
pub use server::{serve, PROTOCOL_VERSION, SERVER_NAME};
pub use session::Host;

/// Start a server on stdin/stdout with the given options. The entry point the
/// `gameboy-mcp` binary is.
pub fn run(options: Options) -> std::io::Result<()> {
    let mut host = Host::new(options);
    // A cartridge named at launch is loaded *before* the first request, so a
    // client's very first `observe` has something to look at. Failure is
    // reported and the server still starts: `load_rom` is a tool, so a typo in
    // a Claude Desktop config is recoverable from inside the session rather
    // than being a server that will not start.
    let rom = host.options.rom.clone();
    if let Some(path) = rom {
        match host.load(&path) {
            Ok(notes) => {
                eprintln!("gameboy-mcp: loaded {}", path.display());
                for n in notes {
                    eprintln!("gameboy-mcp: {n}");
                }
            }
            Err(e) => eprintln!("gameboy-mcp: {e}"),
        }
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(&mut host, stdin.lock(), stdout.lock())
}

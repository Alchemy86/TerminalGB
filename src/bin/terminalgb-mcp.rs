//! `terminalgb-mcp` — the MCP server binary.
//!
//! Everything it does lives in [`terminalgb::mcp`]; this is argument parsing and
//! an exit code. See `docs/mcp.md` for the Claude Desktop and Claude Code
//! config blocks.

// `terminalgb::mcp` is compiled out on `wasm32` (`src/lib.rs`: the transport
// is stdio and the machine needs the APU), and Cargo cannot target-gate a
// `[[bin]]` — so on wasm this binary is an empty shell rather than a build
// error. Same shape as `terminalgb-chiptune`.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match terminalgb::mcp::config::parse(args) {
        Ok(Some(o)) => o,
        // `--help` / `--version` printed and there is nothing to run.
        Ok(None) => return,
        Err(e) => {
            eprintln!("terminalgb-mcp: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = terminalgb::mcp::run(options) {
        eprintln!("terminalgb-mcp: {e}");
        std::process::exit(1);
    }
}

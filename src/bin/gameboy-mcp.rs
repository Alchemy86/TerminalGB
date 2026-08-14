//! `gameboy-mcp` — the MCP server binary.
//!
//! Everything it does lives in [`gameboy::mcp`]; this is argument parsing and
//! an exit code. See `docs/mcp.md` for the Claude Desktop and Claude Code
//! config blocks.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match gameboy::mcp::config::parse(args) {
        Ok(Some(o)) => o,
        // `--help` / `--version` printed and there is nothing to run.
        Ok(None) => return,
        Err(e) => {
            eprintln!("gameboy-mcp: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = gameboy::mcp::run(options) {
        eprintln!("gameboy-mcp: {e}");
        std::process::exit(1);
    }
}

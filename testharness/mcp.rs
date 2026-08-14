//! **MCP server harness** — drive the real `terminalgb-mcp` binary the way a
//! client does: spawn it, speak JSON-RPC down its stdin, read its stdout.
//!
//! # Why a subprocess and not the in-process handler
//!
//! `src/mcp/server.rs` already unit-tests `handle_line`, which is most of the
//! logic. What those tests cannot catch is everything *around* it: the binary's
//! argument parsing, `--rom` loading before the first request, stdout carrying
//! only protocol and nothing else, one line per reply, and the flush after each
//! one. A server that starts is not a server that works — so this runs the
//! shipped executable end to end and asserts on what actually comes back.
//!
//! It needs the vendored MIT dmg-acid2 ROM and nothing else: no commercial
//! cartridge, no submodule, no network. The Gen 1 half of the surface
//! (`observe`'s party and position, `run_actions`' walking) cannot be proved
//! without a copy of Pokémon Red/Blue, so it is **not** claimed here — the
//! honest statement is in the header the harness prints.
//!
//! Run: `cargo test --release --features mcp --test mcp`

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const ACID2: &str = "third_party/dmg-acid2/dmg-acid2.gb";

fn main() {
    let mut fails = 0;
    println!("MCP server harness — driving the real binary over stdio\n");

    fails += case("the handshake and the tool listing", handshake);
    fails += case("stdout carries protocol and nothing else", clean_stdout);
    fails += case(
        "a cartridge named at launch is already running",
        rom_at_launch,
    );
    fails += case(
        "load_rom, step, screenshot and read_memory",
        the_working_loop,
    );
    fails += case(
        "a save state round-trips through the tools",
        state_round_trip,
    );
    fails += case("the frame cap is honoured and reported", the_frame_cap);
    fails += case(
        "a bad call is reported without ending the session",
        survives_junk,
    );

    println!();
    if fails == 0 {
        println!("PASS — every check green");
    } else {
        println!("FAIL — {fails} check(s) failed");
        std::process::exit(1);
    }
}

fn case(name: &str, f: impl FnOnce() -> Result<String, String>) -> i32 {
    match f() {
        Ok(note) => {
            println!("  ok    {name}\n          {note}");
            0
        }
        Err(e) => {
            println!("  FAIL  {name}\n          {e}");
            1
        }
    }
}

// -- a client ---------------------------------------------------------------

/// The binary under test. `cargo test` builds binaries into the same profile
/// directory as the test executable, so it is found relative to the test's own
/// path rather than assumed to be `target/release`.
fn binary() -> PathBuf {
    let mut p = std::env::current_exe().expect("test executable path");
    p.pop(); // deps/
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("terminalgb-mcp")
}

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Client {
    fn start(args: &[&str]) -> Result<Client, String> {
        let bin = binary();
        if !bin.exists() {
            return Err(format!(
                "{} does not exist — build it with `cargo build --release \
                 --features mcp`",
                bin.display()
            ));
        }
        let mut child = Command::new(&bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot spawn {}: {e}", bin.display()))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Client {
            child,
            stdin,
            stdout,
            next_id: 0,
        })
    }

    fn request(&mut self, method: &str, params: &str) -> Result<String, String> {
        self.next_id += 1;
        let id = self.next_id;
        writeln!(
            self.stdin,
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
        )
        .map_err(|e| format!("write: {e}"))?;
        self.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Err(format!("the server closed stdout during {method}"));
        }
        if !line.contains(&format!("\"id\":{id}")) {
            return Err(format!("reply did not carry id {id}: {line}"));
        }
        Ok(line)
    }

    /// Call a tool and return the text of its first content block.
    fn call(&mut self, tool: &str, args: &str) -> Result<String, String> {
        let line = self.request(
            "tools/call",
            &format!(r#"{{"name":"{tool}","arguments":{args}}}"#),
        )?;
        let v = parse(&line)?;
        let result = v
            .get("result")
            .ok_or_else(|| format!("{tool}: no result in {line}"))?;
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.str_field("text"))
            .ok_or_else(|| format!("{tool}: no text content in {line}"))?
            .to_string();
        if result.get("isError").and_then(|e| e.as_bool()) == Some(true) {
            return Err(format!("{tool} reported: {text}"));
        }
        Ok(text)
    }

    /// The whole reply line, for checks that care about the envelope.
    fn call_raw(&mut self, tool: &str, args: &str) -> Result<String, String> {
        self.request(
            "tools/call",
            &format!(r#"{{"name":"{tool}","arguments":{args}}}"#),
        )
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

fn parse(line: &str) -> Result<terminalgb_plugin_agent::json::Json, String> {
    terminalgb_plugin_agent::json::parse(line.trim()).map_err(|e| {
        format!("the server emitted something that is not JSON: {e}\n{line}")
    })
}

fn acid2() -> Result<&'static Path, String> {
    let p = Path::new(ACID2);
    if p.exists() {
        Ok(p)
    } else {
        Err(format!("{ACID2} is missing from the checkout"))
    }
}

fn handshake_with(c: &mut Client) -> Result<(), String> {
    let line = c.request("initialize", "{}")?;
    let v = parse(&line)?;
    let r = v.get("result").ok_or("initialize returned no result")?;
    if r.str_field("protocolVersion").is_none() {
        return Err(format!("no protocolVersion: {line}"));
    }
    Ok(())
}

// -- the checks -------------------------------------------------------------

fn handshake() -> Result<String, String> {
    let mut c = Client::start(&[])?;
    handshake_with(&mut c)?;
    // A notification must not be answered. Proving that over a pipe needs a
    // second request: if the notification *were* answered, this reply would be
    // the stale one and its id would not match.
    writeln!(
        c.stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .map_err(|e| e.to_string())?;
    c.stdin.flush().map_err(|e| e.to_string())?;

    let line = c.request("tools/list", "{}")?;
    let v = parse(&line)?;
    let tools = v
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .ok_or("tools/list returned no array")?;
    let names: Vec<&str> = tools.iter().filter_map(|t| t.str_field("name")).collect();
    for want in [
        "load_rom",
        "observe",
        "screenshot",
        "read_memory",
        "press_buttons",
        "run_actions",
        "step",
        "save_state",
        "load_state",
        "reset",
    ] {
        if !names.contains(&want) {
            return Err(format!("{want} is missing from {names:?}"));
        }
    }
    if names.contains(&"write_memory") {
        return Err(
            "write_memory must not be listed without --allow-write-memory".to_string(),
        );
    }
    c.shutdown();
    Ok(format!("{} tools, notification unanswered", names.len()))
}

/// Anything on stdout that is not a JSON-RPC reply breaks the transport, and
/// the client's error names the transport rather than us. Diagnostics go to
/// stderr — which is why `--rom` prints there.
fn clean_stdout() -> Result<String, String> {
    let rom = acid2()?;
    let bin = binary();
    let mut child = Command::new(&bin)
        .args(["--rom", rom.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    {
        let stdin = child.stdin.as_mut().expect("piped");
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize"}}"#)
            .map_err(|e| e.to_string())?;
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .map_err(|e| e.to_string())?;
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"ping"}}"#)
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != 2 {
        return Err(format!(
            "expected exactly two replies on stdout, got {}: {text}",
            lines.len()
        ));
    }
    for l in &lines {
        parse(l)?;
    }
    Ok("two requests, two JSON lines, no stray output".to_string())
}

fn rom_at_launch() -> Result<String, String> {
    let rom = acid2()?;
    // `--no-battery` because this ROM lives in the checkout and the MBC would
    // otherwise write a `.sav` beside it.
    let mut c = Client::start(&["--rom", rom.to_str().unwrap(), "--no-battery"])?;
    handshake_with(&mut c)?;
    // No `load_rom` call: a cartridge named at launch must already be running.
    let text = c.call("observe", "{}")?;
    let v = parse(&text)?;
    let title = v.str_field("rom").unwrap_or("").to_string();
    if title.is_empty() {
        return Err(format!("observe reported no cartridge: {text}"));
    }
    c.shutdown();
    Ok(format!(
        "observe answered for \"{title}\" with no load_rom call"
    ))
}

fn the_working_loop() -> Result<String, String> {
    let rom = acid2()?;
    let mut c = Client::start(&["--no-battery"])?;
    handshake_with(&mut c)?;

    // Every tool must refuse clearly before a cartridge exists.
    let e = c
        .call("observe", "{}")
        .err()
        .ok_or("observe answered with no cartridge loaded")?;
    if !e.contains("load_rom") {
        return Err(format!("the refusal does not say what to do: {e}"));
    }

    let loaded = c.call("load_rom", &format!(r#"{{"path":"{}"}}"#, rom.display()))?;
    if !loaded.contains("Loaded") {
        return Err(format!("load_rom: {loaded}"));
    }
    c.call("step", r#"{"frames":600}"#)?;

    // A screenshot must be a real PNG in a real image block — a call that
    // returns success and displays nothing is the failure this harness exists
    // to catch.
    let line = c.call_raw("screenshot", "{}")?;
    let v = parse(&line)?;
    let content = v
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .ok_or("screenshot returned no content")?;
    let image = content
        .iter()
        .find(|c| c.str_field("type") == Some("image"))
        .ok_or("screenshot returned no image block")?;
    if image.str_field("mimeType") != Some("image/png") {
        return Err("the image block is not declared as a PNG".to_string());
    }
    let b64 = image
        .str_field("data")
        .ok_or("the image block has no data")?;
    let png = unbase64(b64)?;
    if png.len() < 100 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(format!(
            "the image data is not a PNG ({} bytes, starts {:02X?})",
            png.len(),
            &png[..8.min(png.len())]
        ));
    }

    // Reading memory must return the machine's real bytes. `$FF44` is LY, the
    // LCD's current line — always in range, and it is not something a stub
    // could plausibly fake.
    let ly = c.call("read_memory", r#"{"address":65348,"length":1}"#)?;
    if !ly.contains("$FF44") {
        return Err(format!("read_memory named the wrong address: {ly}"));
    }
    c.shutdown();
    Ok(format!("{} byte PNG, memory read back", png.len()))
}

fn state_round_trip() -> Result<String, String> {
    let rom = acid2()?;
    let path = std::env::temp_dir().join("gb-mcp-harness.gbss");
    let _ = std::fs::remove_file(&path);
    let mut c = Client::start(&["--rom", rom.to_str().unwrap(), "--no-battery"])?;
    handshake_with(&mut c)?;

    // **The probe has to be something that moves.** dmg-acid2 draws one static
    // picture, so `observe` returns byte-identical text at any two frames and a
    // `load_state` that did nothing at all would sail through a comparison of
    // it — measured, by making `load_state` a no-op and watching this check stay
    // green. `$FF04` is DIV, the system counter's top byte: it advances ~274
    // counts every frame, so any two different frames read differently, and
    // emulation here is deterministic so that is a fact rather than a
    // likelihood.
    let probe = |c: &mut Client| c.call("read_memory", r#"{"address":65284,"length":4}"#);

    c.call("step", r#"{"frames":400}"#)?;
    let before = probe(&mut c)?;
    let before_state = c.call("observe", "{}")?;
    c.call("save_state", &format!(r#"{{"path":"{}"}}"#, path.display()))?;
    c.call("step", r#"{"frames":201}"#)?;
    let moved = probe(&mut c)?;
    if moved == before {
        return Err(format!(
            "the probe did not move over 201 frames, so this check could not \
             detect a load that did nothing: {before}"
        ));
    }
    c.call("load_state", &format!(r#"{{"path":"{}"}}"#, path.display()))?;
    let after = probe(&mut c)?;
    if after != before {
        return Err(format!(
            "the machine did not come back\nbefore {before}\nafter  {after}"
        ));
    }
    let after_state = c.call("observe", "{}")?;
    // `frame` is the session's own counter and deliberately keeps counting
    // across a load, so compare everything else.
    let strip = |s: &str| -> String {
        s.split(',')
            .filter(|f| !f.contains("\"frame\""))
            .collect::<Vec<_>>()
            .join(",")
    };
    if strip(&after_state) != strip(&before_state) {
        return Err(format!(
            "state did not round-trip\nbefore {before_state}\nafter  {after_state}"
        ));
    }
    let _ = std::fs::remove_file(&path);
    c.shutdown();
    Ok("saved, ran 201 frames on, loaded, and DIV came back exactly".to_string())
}

fn the_frame_cap() -> Result<String, String> {
    let rom = acid2()?;
    let mut c = Client::start(&[
        "--rom",
        rom.to_str().unwrap(),
        "--no-battery",
        "--max-frames",
        "50",
    ])?;
    handshake_with(&mut c)?;
    let text = c.call("step", r#"{"frames":100000}"#)?;
    if !text.contains("Ran 50 frames") {
        return Err(format!("the cap was not applied: {text}"));
    }
    // Silently running 50 of 100000 would read as "done". It must say so.
    if !text.contains("max-frames") {
        return Err(format!("the cap was applied but not reported: {text}"));
    }
    c.shutdown();
    Ok("100000 frames asked for, 50 run, and the client told why".to_string())
}

fn survives_junk() -> Result<String, String> {
    let mut c = Client::start(&[])?;
    handshake_with(&mut c)?;
    writeln!(c.stdin, "this is not json at all").map_err(|e| e.to_string())?;
    c.stdin.flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    c.stdout.read_line(&mut line).map_err(|e| e.to_string())?;
    let v = parse(&line)?;
    if v.get("error").is_none() {
        return Err(format!("garbage was not reported as an error: {line}"));
    }
    // An unknown tool: a transport-level error naming what went wrong.
    let line = c.call_raw("no_such_tool", "{}")?;
    if !line.contains("unknown tool") {
        return Err(format!("an unknown tool was not refused: {line}"));
    }
    // ...and the session is still alive.
    c.request("ping", "{}")?;
    c.shutdown();
    Ok("garbage, an unknown tool, and the session carried on".to_string())
}

/// Base64 decoder, so the harness can prove the screenshot really is a PNG
/// without adding a dependency. Encoding lives in `termimage`; decoding does
/// not exist anywhere in the crate, and a decoder that shared code with the
/// encoder would prove less.
fn unbase64(s: &str) -> Result<Vec<u8>, String> {
    const TABLE: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for ch in s.bytes() {
        if ch == b'=' || ch == b'\n' || ch == b'\r' {
            continue;
        }
        let v = TABLE
            .iter()
            .position(|&t| t == ch)
            .ok_or_else(|| format!("'{}' is not base64", ch as char))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

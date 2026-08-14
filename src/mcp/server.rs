//! The MCP wire: JSON-RPC 2.0 over stdio, and the tool surface.
//!
//! # Why the surface is eleven tools and not thirty
//!
//! A tool a model cannot tell when to use is worse than no tool: it costs
//! tokens in every request and it invites the wrong call. So each of these
//! answers a question a client genuinely has, and no two answer the same one.
//!
//! | tool | the question |
//! |---|---|
//! | `load_rom` / `reset` | *what am I playing?* |
//! | `observe` | *where am I and what is the game doing?* |
//! | `screenshot` | *what does it actually look like?* — the fallback when there is no address for what you need |
//! | `read_memory` | *what does the game think?* — the reason an agent can be fast instead of squinting at pixels |
//! | `press_buttons` | *do this exact thing* — frame-accurate, works on any cartridge |
//! | `run_actions` | *achieve this* — walk, go to, read through the dialogue; one call is hundreds of frames |
//! | `step` | *let time pass* — a boot, an animation, a cartridge the observer does not know |
//! | `save_state` / `load_state` | *let me try that again* |
//! | `render_audio` | *what would this music sound like on the hardware?* — the only one that needs no cartridge |
//!
//! `write_memory` is an eleventh that exists only when the operator passed
//! `--allow-write-memory`; it is **not listed** otherwise, so a client never
//! sees a tool it cannot use.
//!
//! # The framing
//!
//! MCP over stdio is newline-delimited JSON-RPC 2.0: one object per line on
//! stdin, one per line on stdout. Nothing else may go to stdout — diagnostics
//! go to stderr, which every MCP host collects. That is also why there is no
//! terminal frontend option here (see [`super::config`]).
//!
//! The transport is hand-rolled over [`terminalgb_plugin_agent::json`] for the same reason
//! the test harness hand-rolls PNG and the terminal path hand-rolls deflate: a
//! dependency here is one every embedder inherits. The MCP module is behind the
//! `mcp` cargo feature precisely so nobody who does not want it pays for it.

use std::io::{BufRead, Write};

use crate::input::KeypadKey;
use terminalgb_plugin_agent::json::{self, write_str, Json, Writer};

use super::config::Options;
use super::session::{slot_path, Host};

/// The MCP revision this server implements. Clients send their own in
/// `initialize`; we answer with ours, which is what the spec asks for.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

pub const SERVER_NAME: &str = "terminalgb";

/// JSON-RPC error codes used here. `-32602` is the spec's "invalid params",
/// which is the honest code for "you asked for a tool that needs a cartridge".
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;
const PARSE_ERROR: i64 = -32700;

/// One tool, as it appears in `tools/list`.
struct Tool {
    name: &'static str,
    description: &'static str,
    /// The `inputSchema` object body — properties and required, without the
    /// enclosing `{"type":"object", ...}`, which [`write_tool`] adds.
    schema: &'static str,
}

/// The tool surface. Descriptions are written for a model deciding *which* one
/// to call, so each says what it is for and what it is not for.
const TOOLS: &[Tool] = &[
    Tool {
        name: "load_rom",
        description:
            "Load a Game Boy cartridge from a file path and start it from cold. \
             Replaces whatever was running. Returns the cartridge title and \
             whether structured game state is available for it (Pokémon \
             Red/Blue only; every other cartridge still supports screenshot, \
             read_memory, press_buttons and step).",
        schema: r#""properties":{"path":{"type":"string","description":"Path to a .gb or .gbc file."},"model":{"type":"string","enum":["auto","dmg","cgb"],"description":"Console to run it on. Default auto: read the cartridge's CGB flag."}},"required":["path"]"#,
    },
    Tool {
        name: "reset",
        description: "Reboot the current cartridge from cold, as the console's power \
             switch does. Battery-backed save data survives; save states and \
             anything in RAM do not.",
        schema: r#""properties":{}"#,
    },
    Tool {
        name: "observe",
        description: "Read the game's state as structured data: what the game is doing \
             (title/overworld/dialogue/menu/battle/cutscene), the text \
             currently on screen with what shape of screen it is \
             (screen.kind: no-text/text/dialogue/menu/yes-no/naming-keyboard), \
             the item the menu cursor points at (screen.selected), whether the \
             decode can be trusted (screen.trust — anything but 'ok' means \
             screen.text was withheld rather than guessed), and whether it is \
             waiting for a button. Sprites — people and objects — are drawn \
             over the tilemap and are NOT in screen.text. For \
             Pokémon Red/Blue it also returns the player's map, coordinates and \
             facing, the map's exits, party, badges, money and the battle \
             opponent. Prefer this over screenshot: it is a few hundred bytes \
             where an image is orders of magnitude more, and it is the only way \
             to tell whether an action worked.",
        schema: r#""properties":{}"#,
    },
    Tool {
        name: "screenshot",
        description: "The current frame as a PNG image. Use it when there is no address \
             or structured field for what you need to know — an unfamiliar \
             cartridge, a graphical puzzle, or confirming what the player would \
             actually see. It does not advance time.",
        schema: r#""properties":{"scale":{"type":"integer","minimum":1,"maximum":6,"description":"Nearest-neighbour upscale of the 160x144 frame. 1 is smallest; 2-3 is easier to read at 4-9x the size."}}"#,
    },
    Tool {
        name: "read_memory",
        description:
            "Read bytes of the Game Boy's address space (0x0000-0xFFFF) exactly \
             as the CPU would see them, without perturbing timing. This is what \
             makes an agent fast: one read of a known address beats inferring \
             the same fact from pixels. Returns hex and decimal.",
        schema: r#""properties":{"address":{"type":"integer","minimum":0,"maximum":65535,"description":"Start address. Accepts a number; write hex as decimal (0xD35E = 54110)."},"length":{"type":"integer","minimum":1,"maximum":256,"description":"Bytes to read. Default 1."}},"required":["address"]"#,
    },
    Tool {
        name: "write_memory",
        description:
            "Write bytes into the Game Boy's address space. A debugging tool: it \
             edits the game rather than playing it and can corrupt the save \
             file. Only available when the server was started with \
             --allow-write-memory.",
        schema: r#""properties":{"address":{"type":"integer","minimum":0,"maximum":65535},"bytes":{"type":"array","items":{"type":"integer","minimum":0,"maximum":255},"description":"Byte values to write, starting at address."}},"required":["address","bytes"]"#,
    },
    Tool {
        name: "press_buttons",
        description: "Press buttons with exact frame timing and run the machine until \
             the press has finished, so state read straight afterwards reflects \
             it. Works on any cartridge. Use this for title screens, menus and \
             anything the higher-level actions do not cover. Buttons in one \
             call are pressed in order, not together.",
        schema: r#""properties":{"buttons":{"type":"array","items":{"type":"string","enum":["a","b","start","select","up","down","left","right"]},"description":"Buttons to press, in order."},"times":{"type":"integer","minimum":1,"maximum":64,"description":"Repeat the whole sequence this many times. Default 1."},"hold_frames":{"type":"integer","minimum":1,"maximum":120,"description":"Frames to hold each button. Default 6."},"gap_frames":{"type":"integer","minimum":0,"maximum":240,"description":"Frames of quiet after each press. Default 24 - Pokemon's menu code ignores presses closer together than about that."}},"required":["buttons"]"#,
    },
    Tool {
        name: "run_actions",
        description: "Run a short plan of high-level actions and report what each one \
             achieved. Pokémon Red/Blue only. This is the tool that keeps a \
             session affordable: 'advance_text' holds a whole conversation \
             down, and 'goto' walks to a coordinate around what blocks it - one \
             call covering hundreds of frames instead of one call per button. \
             Every action is bounded and reports honestly when it was stopped \
             by a wall or a cap.",
        schema: r#""properties":{"actions":{"type":"array","description":"Up to 12 actions, run in order.","items":{"type":"object","properties":{"do":{"type":"string","enum":["press","walk","goto","advance_text","wait"],"description":"press: tap a button, with 'button' and optional 'times'. walk: take 'steps' steps in 'dir', stopping at a wall. goto: head for 'x','y' on this map. advance_text: press A through dialogue until it ends. wait: do nothing for 'frames'."},"button":{"type":"string"},"dir":{"type":"string","enum":["up","down","left","right"]},"steps":{"type":"integer"},"times":{"type":"integer"},"x":{"type":"integer"},"y":{"type":"integer"},"frames":{"type":"integer"}},"required":["do"]}}},"required":["actions"]"#,
    },
    Tool {
        name: "step",
        description: "Advance the emulator by a number of frames and do nothing else. \
             The game runs at 59.7275 frames a second, so 60 is about one \
             second. Use it to let a boot sequence, a fade or an animation \
             finish, and on cartridges the higher-level actions do not \
             understand.",
        schema: r#""properties":{"frames":{"type":"integer","minimum":1,"description":"Frames to run. Capped by the server's --max-frames."}},"required":["frames"]"#,
    },
    Tool {
        name: "save_state",
        description: "Snapshot the complete machine to a numbered slot or a file, so a \
             later load_state returns to exactly this point. Slots are \
             namespaced by cartridge and are the same ones the terminal \
             frontend's F5/F8 use.",
        schema: r#""properties":{"slot":{"type":"integer","minimum":0,"maximum":99,"description":"Slot number. Default 1."},"path":{"type":"string","description":"Write to this exact file instead of a slot."}}"#,
    },
    Tool {
        name: "load_state",
        description: "Restore a snapshot taken by save_state. A state from a different \
             cartridge is refused rather than loaded.",
        schema: r#""properties":{"slot":{"type":"integer","minimum":0,"maximum":99,"description":"Slot number. Default 1."},"path":{"type":"string","description":"Read this exact file instead of a slot."}}"#,
    },
    Tool {
        name: "render_audio",
        description: "Render Game Boy sound-chip instructions to a WAV file through the \
             emulated APU — the same chip the accuracy suites score, so the \
             answer is what the hardware would produce, not an imitation. \
             Needs no cartridge. Feed it a score (notes, durations, envelopes, \
             duty, sweep, noise), a register script (NR10-NR52 with timings), \
             or the path of a .vgm log. Returns what was produced — length, \
             peak level, which channels sounded — and optionally the compiled \
             register writes, which is the ground truth the audio comes from. \
             You cannot hear the file; use `registers` and the reported levels \
             to check your work, and give the path to a person to listen to. \
             Score syntax: `tempo 120`, then lines like `ch1 duty 2`, \
             `ch1 env 15 - 3`, `ch1 note C4 1/8`, `ch3 wave <32 hex digits>`, \
             `ch4 noise 4 15 4`, `ch4 hit 1/16`, `ch2 rest 1/4`. Each channel \
             keeps its own cursor from time zero, so parts written one after \
             another play together.",
        schema: r#""properties":{"score":{"type":"string","description":"The source text: a score, or a register script. Mutually exclusive with \"path\"."},"path":{"type":"string","description":"Read the source from this file instead. This is the only way to render a .vgm log."},"out":{"type":"string","description":"Where to write the .wav. Default: a file in the system temp directory."},"rate":{"type":"integer","minimum":4000,"maximum":192000,"description":"Sample rate. Default 44100."},"model":{"type":"string","enum":["dmg","cgb"],"description":"Which console's APU. A `model` line in the source wins. Default dmg."},"seconds":{"type":"number","description":"Override the piece's length."},"registers":{"type":"boolean","description":"Include the compiled register writes in the reply. Default false; they can be long."}}"#,
    },
];

/// The tools this launch exposes. `write_memory` is filtered out unless the
/// operator turned it on, so a client is never shown a tool that will refuse.
fn visible(o: &Options) -> impl Iterator<Item = &'static Tool> + '_ {
    TOOLS
        .iter()
        .filter(move |t| t.name != "write_memory" || o.allow_write_memory)
}

/// Run the server against the given streams until the client closes stdin.
///
/// Split out from the binary so the protocol can be driven in-process by a
/// test — the tests in this module are the same code path a real client takes,
/// byte for byte.
pub fn serve<R: BufRead, W: Write>(
    host: &mut Host,
    input: R,
    mut output: W,
) -> std::io::Result<()> {
    let mut log = host
        .options
        .log
        .as_ref()
        .and_then(|p| std::fs::File::create(p).ok());
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{{\"dir\":\"in\",\"line\":{}}}", quoted(&line));
        }
        let Some(reply) = handle_line(host, &line) else {
            // A notification (no `id`) gets no reply, per JSON-RPC. Answering
            // one is a protocol error that some clients treat as fatal.
            continue;
        };
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{{\"dir\":\"out\",\"line\":{}}}", quoted(&reply));
        }
        writeln!(output, "{reply}")?;
        // Flushed per message: a client is blocked on this reply, and a
        // buffered one is indistinguishable from a hung server.
        output.flush()?;
    }
    host.flush();
    Ok(())
}

fn quoted(s: &str) -> String {
    let mut out = String::new();
    write_str(&mut out, s);
    out
}

/// Handle one line. `None` means "no reply is owed".
pub fn handle_line(host: &mut Host, line: &str) -> Option<String> {
    let msg = match json::parse(line) {
        Ok(v) => v,
        Err(e) => return Some(error_response(&Json::Null, PARSE_ERROR, &format!("{e}"))),
    };
    let id = msg.get("id").cloned().unwrap_or(Json::Null);
    let method = msg.str_field("method").unwrap_or_default().to_string();
    let params = msg.get("params").cloned().unwrap_or(Json::Null);

    // A JSON-RPC notification has no `id`. `notifications/initialized` is the
    // one every MCP client sends and the one a naive server answers by mistake.
    msg.get("id")?;

    match method.as_str() {
        "initialize" => Some(result_response(&id, &initialize_result())),
        "ping" => Some(result_response(&id, "{}")),
        "tools/list" => Some(result_response(&id, &tools_list(&host.options))),
        "tools/call" => Some(call_tool(host, &id, &params)),
        // `resources/list` and `prompts/list` are asked for by clients that do
        // not read the capability set. An empty list is the correct answer and
        // is friendlier than an error in a log nobody reads.
        "resources/list" => Some(result_response(&id, r#"{"resources":[]}"#)),
        "prompts/list" => Some(result_response(&id, r#"{"prompts":[]}"#)),
        other => Some(error_response(
            &id,
            METHOD_NOT_FOUND,
            &format!("unknown method '{other}'"),
        )),
    }
}

fn initialize_result() -> String {
    let mut w = Writer::new();
    w.begin_obj();
    w.str_field("protocolVersion", PROTOCOL_VERSION);
    w.obj_field("capabilities");
    w.obj_field("tools");
    w.bool_field("listChanged", false);
    w.end_obj();
    w.end_obj();
    w.obj_field("serverInfo");
    w.str_field("name", SERVER_NAME);
    w.str_field("version", env!("CARGO_PKG_VERSION"));
    w.end_obj();
    w.str_field(
        "instructions",
        "Drives a Game Boy emulator. Start with `observe` to see what the game \
         is doing; it is far cheaper than `screenshot` and is the only way to \
         tell whether an action worked. On Pokémon Red/Blue prefer \
         `run_actions` — one call walks a route or reads a whole conversation. \
         `press_buttons` is the frame-accurate fallback and works on any \
         cartridge. `step` just lets time pass.",
    );
    w.end_obj();
    w.finish()
}

fn tools_list(o: &Options) -> String {
    let mut out = String::from(r#"{"tools":["#);
    for (i, t) in visible(o).enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        write_str(&mut out, t.name);
        out.push_str(",\"description\":");
        write_str(&mut out, t.description);
        out.push_str(",\"inputSchema\":{\"type\":\"object\",");
        out.push_str(t.schema);
        out.push_str("}}");
    }
    out.push_str("]}");
    out
}

// -- tool dispatch ---------------------------------------------------------

/// The result of a tool: text, or text plus an image.
struct ToolOut {
    text: String,
    /// base64 PNG, when the tool returns a picture.
    png: Option<String>,
    is_error: bool,
}

impl ToolOut {
    fn text(s: impl Into<String>) -> ToolOut {
        ToolOut {
            text: s.into(),
            png: None,
            is_error: false,
        }
    }
    fn err(s: impl Into<String>) -> ToolOut {
        ToolOut {
            text: s.into(),
            png: None,
            is_error: true,
        }
    }
}

fn call_tool(host: &mut Host, id: &Json, params: &Json) -> String {
    let name = params.str_field("name").unwrap_or_default().to_string();
    let args = params.get("arguments").cloned().unwrap_or(Json::Null);
    if !visible(&host.options).any(|t| t.name == name) {
        let hint = if name == "write_memory" {
            " — this server was not started with --allow-write-memory"
        } else {
            ""
        };
        return error_response(
            id,
            INVALID_PARAMS,
            &format!("unknown tool '{name}'{hint}"),
        );
    }
    let out = run_tool(host, &name, &args);
    // A tool that failed is reported as a *successful* JSON-RPC call carrying
    // `isError` — that is what the MCP spec asks for, and it is what lets the
    // model read the failure and try something else instead of the host
    // treating it as a transport fault.
    let mut w = Writer::new();
    w.begin_obj();
    w.arr_field("content");
    w.begin_obj();
    w.str_field("type", "text");
    w.str_field("text", &out.text);
    w.end_obj();
    if let Some(png) = &out.png {
        w.begin_obj();
        w.str_field("type", "image");
        w.str_field("data", png);
        w.str_field("mimeType", "image/png");
        w.end_obj();
    }
    w.end_arr();
    w.bool_field("isError", out.is_error);
    w.end_obj();
    result_response(id, &w.finish())
}

fn run_tool(host: &mut Host, name: &str, args: &Json) -> ToolOut {
    match name {
        "load_rom" => {
            let Some(path) = args.str_field("path") else {
                return ToolOut::err("load_rom needs a \"path\"");
            };
            let path = std::path::PathBuf::from(shellexpand_home(path));
            if let Some(m) = args.str_field("model") {
                match crate::mode::Model::parse(m) {
                    Some(model) => host.options.model = model,
                    None => {
                        return ToolOut::err(format!(
                            "unknown model '{m}' — use auto, dmg or cgb"
                        ))
                    }
                }
            }
            match host.load(&path) {
                Ok(notes) => {
                    let s = host.session.as_ref().expect("just loaded");
                    let mut msg = format!(
                        "Loaded \"{}\" from {}.\nStructured game state: {}.",
                        s.title(),
                        path.display(),
                        if s.is_supported() {
                            "available (Pokémon Red/Blue)"
                        } else {
                            "not available for this cartridge — use screenshot \
                             and read_memory"
                        }
                    );
                    for n in notes {
                        msg.push('\n');
                        msg.push_str(&n);
                    }
                    ToolOut::text(msg)
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "reset" => {
            let o = host.options.clone();
            match host.need().and_then(|s| s.reset(&o)) {
                Ok(()) => ToolOut::text("Cartridge rebooted from cold."),
                Err(e) => ToolOut::err(e),
            }
        }
        "observe" => {
            let allow = host.options.allow_write_memory;
            match host.need() {
                Ok(s) => ToolOut::text(s.observe_json(allow)),
                Err(e) => ToolOut::err(e),
            }
        }
        "screenshot" => {
            let scale = args
                .u64_field("scale")
                .unwrap_or(host.options.screenshot_scale as u64)
                as u32;
            match host.need() {
                Ok(s) => {
                    let png = s.screenshot_png(scale);
                    let mut b64 = String::new();
                    crate::termimage::base64_into(&png, &mut b64);
                    let (w, h) = (160 * scale.clamp(1, 6), 144 * scale.clamp(1, 6));
                    ToolOut {
                        text: format!("Frame {} — {w}x{h} PNG.", s.frame_count()),
                        png: Some(b64),
                        is_error: false,
                    }
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "read_memory" => {
            let Some(address) = args.u64_field("address") else {
                return ToolOut::err("read_memory needs an \"address\"");
            };
            if address > 0xFFFF {
                return ToolOut::err(format!(
                    "address {address} is outside the Game Boy's 0-65535 address space"
                ));
            }
            let len = args.u64_field("length").unwrap_or(1).clamp(1, 256) as usize;
            let allow = host.options.allow_write_memory;
            match host.need() {
                Ok(s) => {
                    let bytes = s.read_memory(address as u16, len, allow);
                    let hex: Vec<String> =
                        bytes.iter().map(|b| format!("{b:02X}")).collect();
                    let dec: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                    ToolOut::text(format!(
                        "${address:04X}..${:04X}\nhex: {}\ndec: {}",
                        address as usize + len - 1,
                        hex.join(" "),
                        dec.join(" ")
                    ))
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "write_memory" => {
            let Some(address) = args.u64_field("address") else {
                return ToolOut::err("write_memory needs an \"address\"");
            };
            let Some(items) = args.get("bytes").and_then(|b| b.as_array()) else {
                return ToolOut::err("write_memory needs a \"bytes\" array");
            };
            let bytes: Vec<u8> = items
                .iter()
                .filter_map(|v| v.as_u64())
                .map(|v| (v & 0xFF) as u8)
                .collect();
            if bytes.len() != items.len() {
                return ToolOut::err("every entry in \"bytes\" must be a number 0-255");
            }
            let allow = host.options.allow_write_memory;
            match host.need() {
                Ok(s) => {
                    let wrote = s.write_memory(address as u16, &bytes, allow);
                    // The count comes back from the capability-checked handle,
                    // so a write that was refused reports as refused rather
                    // than as "wrote 4 bytes" with nothing changed.
                    if wrote != bytes.len() {
                        return ToolOut::err(format!(
                            "only {wrote} of {} byte(s) were written — this \
                             server was not started with --allow-write-memory",
                            bytes.len()
                        ));
                    }
                    ToolOut::text(format!(
                        "Wrote {wrote} byte(s) at ${address:04X}. The game's own \
                         state is now edited, not played."
                    ))
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "press_buttons" => {
            let Some(items) = args.get("buttons").and_then(|b| b.as_array()) else {
                return ToolOut::err("press_buttons needs a \"buttons\" array");
            };
            let mut keys = Vec::new();
            for v in items {
                let Some(s) = v.as_str() else {
                    return ToolOut::err("every entry in \"buttons\" must be a string");
                };
                match KeypadKey::parse(s) {
                    Some(k) => keys.push(k),
                    None => {
                        return ToolOut::err(format!(
                            "unknown button '{s}' — use a, b, start, select, up, \
                             down, left or right"
                        ))
                    }
                }
            }
            if keys.is_empty() {
                return ToolOut::err("\"buttons\" was empty — nothing to press");
            }
            let times = args.u64_field("times").unwrap_or(1).clamp(1, 64) as u32;
            let hold = args
                .u64_field("hold_frames")
                .unwrap_or(crate::input::DEFAULT_HOLD as u64)
                .clamp(1, 120) as u16;
            let gap = args
                .u64_field("gap_frames")
                .unwrap_or(terminalgb_plugin_agent::action::MENU_GAP as u64)
                .min(240) as u16;
            let o = host.options.clone();
            match host.need() {
                Ok(s) => {
                    let frames = s.press(&keys, times, hold, gap, &o);
                    let names: Vec<&str> = keys.iter().map(|k| k.name()).collect();
                    ToolOut::text(format!(
                        "Pressed {} x{times} ({hold}f hold, {gap}f gap) over {frames} \
                         frames. Now at frame {}.",
                        names.join(", "),
                        s.frame_count()
                    ))
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "run_actions" => {
            let Some(items) = args.get("actions").and_then(|a| a.as_array()) else {
                return ToolOut::err("run_actions needs an \"actions\" array");
            };
            let o = host.options.clone();
            let supported = host.session.as_ref().map(|s| s.is_supported());
            match host.need() {
                Ok(s) => {
                    let (outcomes, errors, frames, aborted) = s.run_actions(items, &o);
                    if outcomes.is_empty() && !errors.is_empty() {
                        return ToolOut::err(format!(
                            "no usable actions: {}",
                            errors.join("; ")
                        ));
                    }
                    let mut msg = String::new();
                    for out in &outcomes {
                        msg.push_str(&format!("{} — {}\n", out.action, out.result));
                    }
                    if let Some(why) = aborted {
                        msg.push_str(&format!("stopped: {why}\n"));
                    }
                    for e in &errors {
                        msg.push_str(&format!("ignored: {e}\n"));
                    }
                    msg.push_str(&format!(
                        "{frames} frames run; now at frame {}.",
                        s.frame_count()
                    ));
                    if supported == Some(false) {
                        msg.push_str(
                            "\nNote: walk and goto do nothing on this cartridge — \
                             structured game state is Pokémon Red/Blue only.",
                        );
                    }
                    ToolOut::text(msg)
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "step" => {
            let Some(frames) = args.u64_field("frames") else {
                return ToolOut::err("step needs \"frames\"");
            };
            let want = frames.min(u32::MAX as u64) as u32;
            let o = host.options.clone();
            match host.need() {
                Ok(s) => {
                    let ran = s.step(want, &o);
                    let capped = if ran < want {
                        format!(
                            " (asked for {want}; the server's --max-frames is {})",
                            o.max_frames
                        )
                    } else {
                        String::new()
                    };
                    ToolOut::text(format!(
                        "Ran {ran} frames{capped}. Now at frame {}.",
                        s.frame_count()
                    ))
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "save_state" | "load_state" => {
            let dir = host.options.state_dir.clone();
            let explicit = args.str_field("path").map(shellexpand_home);
            let slot = args.u64_field("slot").unwrap_or(1).min(99) as u32;
            let saving = name == "save_state";
            match host.need() {
                Ok(s) => {
                    let path = match explicit {
                        Some(p) => std::path::PathBuf::from(p),
                        None => slot_path(dir.as_deref(), s.title(), slot),
                    };
                    if saving {
                        match s.save_state(&path) {
                            Ok(n) => ToolOut::text(format!(
                                "Saved {n} bytes to {} (frame {}).",
                                path.display(),
                                s.frame_count()
                            )),
                            Err(e) => ToolOut::err(e),
                        }
                    } else {
                        match s.load_state(&path) {
                            Ok(()) => ToolOut::text(format!(
                                "Restored the machine from {}.",
                                path.display()
                            )),
                            Err(e) => ToolOut::err(e),
                        }
                    }
                }
                Err(e) => ToolOut::err(e),
            }
        }
        "render_audio" => render_audio(args),
        other => ToolOut::err(format!("unknown tool '{other}'")),
    }
}

/// Compile chiptune instructions and render them through the emulated APU.
///
/// Deliberately independent of the session: there is no cartridge involved, so
/// this works on a server that has never been given a ROM. It is a thin
/// wrapper over [`crate::chiptune`] and must stay one — anything that starts
/// deciding what music *means* belongs in the `terminalgb-chiptune` crate.
fn render_audio(args: &Json) -> ToolOut {
    use terminalgb_chiptune::{compile_auto, track::Model};

    let model = match args.str_field("model") {
        Some(m) => match Model::parse(m) {
            Some(m) => m,
            None => return ToolOut::err(format!("unknown model '{m}' — dmg or cgb")),
        },
        None => Model::Dmg,
    };

    let (bytes, source) = match (args.str_field("score"), args.str_field("path")) {
        (Some(_), Some(_)) => {
            return ToolOut::err("give either \"score\" or \"path\", not both")
        }
        (Some(s), None) => (s.as_bytes().to_vec(), "the score given".to_string()),
        (None, Some(p)) => {
            let p = std::path::PathBuf::from(shellexpand_home(p));
            match std::fs::read(&p) {
                Ok(b) => (b, p.display().to_string()),
                Err(e) => return ToolOut::err(format!("reading {}: {e}", p.display())),
            }
        }
        (None, None) => return ToolOut::err("render_audio needs \"score\" or \"path\""),
    };

    let mut track = match compile_auto(&bytes, model) {
        Ok(t) => t,
        Err(e) => return ToolOut::err(e),
    };
    if let Some(s) = args.get("seconds").and_then(|v| v.as_f64()) {
        if !(s.is_finite() && s > 0.0 && s < 3600.0) {
            return ToolOut::err("\"seconds\" must be between 0 and 3600");
        }
        track.duration = (s * terminalgb_chiptune::CLOCK_HZ as f64).round() as u64;
        track.writes.retain(|w| w.at <= track.duration);
    }
    let rate = args.u64_field("rate").unwrap_or(44_100);
    if !(4_000..=192_000).contains(&rate) {
        return ToolOut::err("\"rate\" must be between 4000 and 192000");
    }
    let rate = rate as u32;

    let out = match args.str_field("out") {
        Some(p) => std::path::PathBuf::from(shellexpand_home(p)),
        None => std::env::temp_dir().join("terminalgb-chiptune-render.wav"),
    };

    let opts = crate::chiptune::RenderOptions {
        sample_rate: rate,
        model: track.model,
        ..Default::default()
    };
    let samples = crate::chiptune::render(&track, opts);
    if let Err(e) = terminalgb_chiptune::wav::write(&out, &samples, rate) {
        return ToolOut::err(format!("writing {}: {e}", out.display()));
    }

    let peak = samples
        .iter()
        .fold(0f32, |m, &(l, r)| m.max(l.abs()).max(r.abs()));
    let rms = (samples
        .iter()
        .map(|&(l, r)| (l as f64) * (l as f64) + (r as f64) * (r as f64))
        .sum::<f64>()
        / (2.0 * samples.len().max(1) as f64))
        .sqrt();
    let mut used = [false; 4];
    for w in &track.writes {
        match w.reg {
            0xFF10..=0xFF14 => used[0] = true,
            0xFF16..=0xFF19 => used[1] = true,
            0xFF1A..=0xFF1E | 0xFF30..=0xFF3F => used[2] = true,
            0xFF20..=0xFF23 => used[3] = true,
            _ => {}
        }
    }
    let names = ["square 1", "square 2", "wave", "noise"];
    let live: Vec<&str> = names
        .iter()
        .zip(used)
        .filter(|(_, u)| *u)
        .map(|(n, _)| *n)
        .collect();

    let mut msg = format!(
        "Rendered {source} on a {} through the emulated APU.\n\
         Wrote {}\n\
         {:.3} s, {} samples at {} Hz, {} register writes.\n\
         Peak {peak:.3}, RMS {rms:.4}. Channels used: {}.",
        track.model.name().to_uppercase(),
        out.display(),
        track.seconds(),
        samples.len(),
        rate,
        track.writes.len(),
        if live.is_empty() {
            "none".to_string()
        } else {
            live.join(", ")
        },
    );
    if peak == 0.0 {
        msg.push_str(
            "\n\nWARNING: the render is silent. Check that the APU is powered on \
             (NR52 bit 7), that NR50 and NR51 route the channel to an output, and \
             that the channel's DAC is on — NRx2 non-zero, or NR30 bit 7 for the \
             wave channel. A score gets the power-on for free unless it says \
             `power off`.",
        );
    }
    if args
        .get("registers")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        msg.push_str("\n\nCompiled register writes:\n");
        msg.push_str(&track.to_register_script());
    }
    ToolOut::text(msg)
}

/// `~` is the one thing a person types into a config file that a bare
/// `PathBuf::from` gets wrong, and the failure ("No such file or directory:
/// ~/roms/x.gb") reads like the file is missing.
fn shellexpand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{rest}", home.to_string_lossy());
        }
    }
    p.to_string()
}

// -- JSON-RPC envelopes ----------------------------------------------------

fn write_id(out: &mut String, id: &Json) {
    match id {
        Json::Str(s) => write_str(out, s),
        Json::Num(n) => {
            // Ids are usually integers and must come back byte-identical
            // enough to match: print 3, not 3.0.
            if n.fract() == 0.0 && n.abs() < 9e15 {
                out.push_str(&format!("{}", *n as i64));
            } else {
                out.push_str(&format!("{n}"));
            }
        }
        _ => out.push_str("null"),
    }
}

fn result_response(id: &Json, result_json: &str) -> String {
    let mut out = String::from("{\"jsonrpc\":\"2.0\",\"id\":");
    write_id(&mut out, id);
    out.push_str(",\"result\":");
    out.push_str(result_json);
    out.push('}');
    out
}

fn error_response(id: &Json, code: i64, message: &str) -> String {
    let mut out = String::from("{\"jsonrpc\":\"2.0\",\"id\":");
    write_id(&mut out, id);
    out.push_str(&format!(",\"error\":{{\"code\":{code},\"message\":"));
    write_str(&mut out, message);
    out.push_str("}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host::new(Options::default())
    }

    /// Every reply this server can produce must be parseable JSON — a
    /// hand-rolled encoder that emits a stray comma takes the whole session
    /// down, and the client's error will name the transport, not us.
    fn json_of(s: &str) -> Json {
        json::parse(s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
    }

    #[test]
    fn initialize_answers_with_a_protocol_version_and_tools_capability() {
        let mut h = host();
        let r = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .expect("a request is owed a reply");
        let v = json_of(&r);
        assert_eq!(v.str_field("jsonrpc"), Some("2.0"));
        let result = v.get("result").expect("result");
        assert_eq!(result.str_field("protocolVersion"), Some(PROTOCOL_VERSION));
        assert!(result
            .get("capabilities")
            .and_then(|c| c.get("tools"))
            .is_some());
        assert_eq!(
            result.get("serverInfo").and_then(|s| s.str_field("name")),
            Some(SERVER_NAME)
        );
        // The id must come back as the integer it was sent as.
        assert!(r.contains("\"id\":1"), "{r}");
    }

    /// A notification has no `id` and must get **no** reply. Answering
    /// `notifications/initialized` is the classic way to break a client that
    /// treats an unexpected response as a protocol fault.
    #[test]
    fn a_notification_is_never_answered() {
        let mut h = host();
        assert!(handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
    }

    #[test]
    fn every_listed_tool_has_a_description_and_an_object_schema() {
        let mut h = host();
        let r = handle_line(&mut h, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .unwrap();
        let v = json_of(&r);
        let tools = v
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .expect("tools array");
        assert!(!tools.is_empty());
        for t in tools {
            let name = t.str_field("name").expect("name");
            let desc = t.str_field("description").expect("description");
            assert!(
                desc.len() > 60,
                "{name}: a description a model cannot act on is worse than no tool"
            );
            let schema = t.get("inputSchema").expect("inputSchema");
            assert_eq!(schema.str_field("type"), Some("object"), "{name}");
            assert!(schema.get("properties").is_some(), "{name}");
        }
    }

    /// The point of the flag: a client must not be shown a tool that will
    /// refuse it.
    #[test]
    fn write_memory_is_absent_from_the_listing_unless_it_was_enabled() {
        let names = |o: Options| -> Vec<String> {
            let mut h = Host::new(o);
            let r =
                handle_line(&mut h, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
                    .unwrap();
            json_of(&r)
                .get("result")
                .and_then(|r| r.get("tools"))
                .and_then(|t| t.as_array())
                .unwrap()
                .iter()
                .map(|t| t.str_field("name").unwrap().to_string())
                .collect()
        };
        let off = names(Options::default());
        assert!(!off.contains(&"write_memory".to_string()), "{off:?}");
        assert!(off.contains(&"read_memory".to_string()));
        let on = names(Options {
            allow_write_memory: true,
            ..Options::default()
        });
        assert!(on.contains(&"write_memory".to_string()), "{on:?}");

        // ...and calling it anyway says why, rather than "unknown tool".
        let mut h = host();
        let r = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"write_memory","arguments":{"address":1,"bytes":[2]}}}"#,
        )
        .unwrap();
        assert!(r.contains("--allow-write-memory"), "{r}");
    }

    /// A tool that fails must come back as a *successful* call carrying
    /// `isError`, so the model reads the message and adapts. A JSON-RPC error
    /// is a transport fault and most clients do not show it to the model.
    #[test]
    fn a_failing_tool_reports_through_the_content_not_the_transport() {
        let mut h = host();
        let r = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"observe","arguments":{}}}"#,
        )
        .unwrap();
        let v = json_of(&r);
        let result = v.get("result").expect("a result, not an error");
        assert_eq!(result.get("isError").and_then(|e| e.as_bool()), Some(true));
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.str_field("text"))
            .unwrap();
        assert!(text.contains("load_rom"), "{text}");
    }

    #[test]
    fn an_unknown_method_is_a_transport_error_with_the_id_echoed() {
        let mut h = host();
        let r = handle_line(
            &mut h,
            r#"{"jsonrpc":"2.0","id":"abc","method":"nonsense/thing"}"#,
        )
        .unwrap();
        let v = json_of(&r);
        assert_eq!(
            v.get("error").and_then(|e| e.as_f64_code()),
            Some(METHOD_NOT_FOUND)
        );
        assert_eq!(v.get("id").and_then(|i| i.as_str()), Some("abc"));
    }

    #[test]
    fn a_malformed_line_does_not_take_the_session_down() {
        let mut h = host();
        let r = handle_line(&mut h, "{not json").unwrap();
        let v = json_of(&r);
        assert!(v.get("error").is_some());
        // ...and the next request still works.
        assert!(
            handle_line(&mut h, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
                .unwrap()
                .contains("result")
        );
    }

    /// Small helper so the assertion above reads as an error code rather than
    /// a float.
    trait ErrCode {
        fn as_f64_code(&self) -> Option<i64>;
    }
    impl ErrCode for Json {
        fn as_f64_code(&self) -> Option<i64> {
            self.get("code").and_then(|c| c.as_f64()).map(|c| c as i64)
        }
    }
}

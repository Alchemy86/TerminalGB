//! The emulator side of the MCP server: one cartridge, and the operations the
//! tools map onto.
//!
//! # This file re-implements nothing
//!
//! Perception is [`terminalgb_plugin_agent::observe`] — the same `Gen1Observer` and
//! `Screen` reader the Claude-plays substrate uses, reached through the same
//! capability-checked [`Machine`](crate::plugin::Machine) handle. High-level
//! movement is [`terminalgb_plugin_agent::action::Executor`], unchanged. This module owns
//! the [`Gameboy`], the clock, and the host resources (files, a sound device, a
//! video sink) — nothing else.
//!
//! That is the whole reason the MCP surface is small: the hard parts already
//! existed, and the honest job here was to expose them, not to rebuild them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::gameboy::Gameboy;
use crate::input::KeypadKey;
use crate::plugin::Capability;
use terminalgb_plugin_agent::action::{plan_from_json, Executor, Limits, Outcome};
use terminalgb_plugin_agent::json::{Json, Writer};
use terminalgb_plugin_agent::observe::{classify, Gen1Observer, Screen, Trust};

use super::config::{Options, Speed};

/// The DMG's true frame period: 70224 T-cycles at 4.194304 MHz.
pub const FRAME_PERIOD: Duration = Duration::from_nanos(16_742_706);

/// What the tools are granted over the machine.
///
/// **`WRITE_MEMORY` is absent unless the operator asked for it.** The agent
/// plugin makes the same choice for the same reason: a client that can poke RAM
/// is editing the game rather than playing it, and the save it leaves behind is
/// a corrupted one. `--allow-write-memory` turns the server into a debugger,
/// which is a different and deliberate thing.
fn grant(allow_write: bool) -> Capability {
    let base = Capability::OBSERVE
        .union(Capability::READ_MEMORY)
        .union(Capability::CONTROL_INPUT);
    if allow_write {
        base.union(Capability::WRITE_MEMORY)
    } else {
        base
    }
}

/// A loaded cartridge and everything attached to it.
pub struct Session {
    gb: Gameboy,
    /// Whether the cartridge is one [`Gen1Observer`] understands. The observer
    /// itself is a one-field value built where it is used — holding it as a
    /// field would borrow `self` for the whole of every `with_machine` call.
    supported: bool,
    exec: Executor,
    rom_path: Option<PathBuf>,
    title: String,
    /// Emulated frames since this cartridge was loaded. The clock every tool
    /// reports against, so a client can tell whether anything advanced.
    frame: u64,
    /// Wall clock the pacing deadline is measured from, and the deadline
    /// itself. A deadline accumulator, never `= now()` — resetting a pacing
    /// timer to `now` silently charges the period for the work as well, which
    /// is the bug `docs/audio.md` records twice.
    next_frame_at: Instant,
    video: crate::video::VideoOut,
    #[cfg(feature = "audio")]
    _audio: Option<cpal::Stream>,
    /// Notes produced while loading — plugins that attached, plugins that did
    /// not, a video sink with no reader yet. Handed to the client verbatim so
    /// an inert attachment is never invisible.
    notes: Vec<String>,
}

/// Every plugin this project ships, whether or not this binary was built with
/// it: `(name, cargo feature)`.
///
/// Listing the *absent* ones is the whole point: it is what turns "unknown
/// plugin" into "rebuild with `--features …`". Mirrors the terminal frontend's
/// registry; a third-party plugin adds a row here only if it wants the same
/// courtesy, and works without one.
const REGISTRY: &[(&str, &str)] = &[
    ("trainer", "plugin-trainer"),
    ("gen1-link", "plugin-gen1-link"),
    ("gen1-battle", "plugin-battle-control"),
    // `agent` is deliberately absent: it is refused rather than missing, so
    // "rebuild with --features plugin-agent" would be the wrong advice.
];

/// The whole server state: the options, and a cartridge once there is one.
pub struct Host {
    pub options: Options,
    pub session: Option<Session>,
}

impl Host {
    pub fn new(options: Options) -> Host {
        Host {
            options,
            session: None,
        }
    }

    /// Load a cartridge, replacing whatever was running.
    pub fn load(&mut self, path: &Path) -> Result<Vec<String>, String> {
        let data = std::fs::read(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if data.len() < 0x150 {
            return Err(format!(
                "{} is {} bytes — too short to hold a cartridge header",
                path.display(),
                data.len()
            ));
        }
        // Flush the outgoing cartridge's battery before it is dropped: `Drop`
        // is not a shutdown hook in this codebase and never has been (see
        // `src/mbc/battery.rs`).
        if let Some(s) = self.session.as_mut() {
            s.gb.flush_battery();
            s.video.flush();
        }
        self.session = None;

        // `None` for the path is what makes `--no-battery` mean it: the MBC
        // only writes a `.sav` when it knows where the ROM came from.
        let save_path = if self.options.battery {
            Some(path.to_path_buf())
        } else {
            None
        };
        let mut gb =
            Gameboy::try_new_with_model(data.clone(), save_path, self.options.model)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        // The operator's declaration about the picture, applied before the
        // first frame — see `Options::pixels`.
        gb.set_render_pixels(self.options.pixels);
        let mut session = Session {
            title: gb.rom_title(),
            gb,
            supported: false,
            exec: Executor::new(Limits::default()),
            rom_path: Some(path.to_path_buf()),
            frame: 0,
            next_frame_at: Instant::now(),
            video: crate::video::VideoOut::disabled(),
            #[cfg(feature = "audio")]
            _audio: None,
            notes: Vec::new(),
        };
        // The observer only reports Gen 1 state for a cartridge it recognises
        // by title *and* header checksum — a hack or a translation with a
        // shifted layout is refused rather than reported wrongly.
        session.supported = terminalgb_plugin_trainer::Game::detect(
            &session.title,
            session.gb.global_checksum(),
        )
        .is_some();
        session.attach_plugins(&self.options, &data);
        session.open_video(&self.options);
        session.open_audio(&self.options);
        let notes = session.notes.clone();
        self.session = Some(session);
        Ok(notes)
    }

    /// The session, or the error a tool should report when there is no
    /// cartridge. One place, so every tool says the same thing.
    pub fn need(&mut self) -> Result<&mut Session, String> {
        match self.session.as_mut() {
            Some(s) => Ok(s),
            None => Err("no cartridge loaded — call load_rom first, or start \
                         the server with --rom"
                .to_string()),
        }
    }

    pub fn flush(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.gb.flush_battery();
            s.gb.flush_plugins();
            s.video.flush();
        }
    }
}

impl Session {
    /// Attach the plugins the operator named, in order, with a note per name.
    ///
    /// The same shape as the terminal frontend's `attach_named`, and for the
    /// same reason: one `if`, gated on the feature that brings the crate in, is
    /// a third-party plugin's whole integration. `None` from `attach_one` means
    /// "no plugin by that name in this build", which is told apart from "that is
    /// not a plugin" — different problems, different fixes.
    fn attach_plugins(&mut self, o: &Options, rom: &[u8]) {
        if let Some(msg) = self.gb.open_plugin_trace() {
            self.notes.push(msg);
        }
        for name in &o.plugins {
            let note = match self.attach_one(name, rom) {
                Some(note) => note,
                None => match REGISTRY.iter().find(|(n, _)| n == name) {
                    Some((_, feature)) => format!(
                        "plugin {name} is not built into this binary — rebuild \
                         with --features {feature}"
                    ),
                    None => format!("unknown plugin '{name}'"),
                },
            };
            self.notes.push(note);
        }
    }

    /// Build and attach one named plugin. `None` = not in this build.
    fn attach_one(&mut self, name: &str, rom: &[u8]) -> Option<String> {
        let say = |r: Result<String, String>, name: &str| match r {
            Ok(msg) => format!("plugin {msg}"),
            Err(e) => format!("plugin {name}: {e}"),
        };

        #[cfg(feature = "plugin-trainer")]
        if name == terminalgb_plugin_trainer::NAME {
            let mut p = terminalgb_plugin_trainer::TrainerPlugin::new(rom.to_vec());
            // Active from the start, unlike the TUI's: there is no panel to
            // switch on here, so an attached-but-inert plugin would only look
            // broken.
            p.set_active(true);
            return Some(say(
                self.gb
                    .add_plugin(Box::new(p))
                    .map(|()| "trainer attached".to_string())
                    .map_err(|e| e.to_string()),
                name,
            ));
        }

        #[cfg(feature = "plugin-gen1-link")]
        if name == terminalgb_plugin_gen1_link::NAME {
            let path = std::env::var("GB_GEN1_TRADE").ok();
            return Some(say(
                terminalgb_plugin_gen1_link::Gen1LinkPartner::from_trade_file(
                    rom.to_vec(),
                    path.as_deref().map(Path::new),
                )
                .map_err(|e| e.to_string())
                .and_then(|p| {
                    let offer = p.offer().to_string();
                    self.gb
                        .add_plugin(Box::new(p))
                        .map(|()| format!("gen1-link attached, offering {offer}"))
                        .map_err(|e| e.to_string())
                }),
                name,
            ));
        }

        #[cfg(feature = "plugin-battle-control")]
        if name == terminalgb_plugin_battle_control::NAME {
            let path = std::env::var("GB_BATTLES").ok();
            return Some(say(
                terminalgb_plugin_battle_control::BattlePlugin::from_preset_file(
                    rom.to_vec(),
                    path.as_deref().map(Path::new),
                )
                .map_err(|e| e.to_string())
                .and_then(|p| {
                    let n = p.preset_names().len();
                    self.gb
                        .add_plugin(Box::new(p))
                        .map(|()| format!("gen1-battle attached, {n} presets"))
                        .map_err(|e| e.to_string())
                }),
                name,
            ));
        }

        // The agent plugin is recognised and **refused**, which is different
        // from absent: an MCP client already *is* the agent, so attaching that
        // one too would put two things on the controller, each undoing the
        // other's plan. Saying so beats a silent no-op or a "not built in"
        // message that sends someone off rebuilding.
        if name == terminalgb_plugin_agent::NAME {
            let _ = rom;
            return Some(format!(
                "plugin {name}: not available here — an MCP client is already \
                 the agent, and two of them share one controller"
            ));
        }
        None
    }

    fn open_video(&mut self, o: &Options) {
        let Some(dest) = o.video_out.as_ref() else {
            return;
        };
        // `-` is stdout, and stdout is the JSON-RPC wire. Interleaving 92 KB of
        // raw pixels with protocol frames does not degrade the session, it ends
        // it — so this is refused with the thing to do instead.
        if dest.trim() == "-" {
            self.notes.push(
                "video-out: stdout is the MCP transport — write to a file or a \
                 FIFO instead (mkfifo /tmp/gb.raw && --video-out /tmp/gb.raw)"
                    .to_string(),
            );
            return;
        }
        let Some(parsed) = crate::video::Destination::parse(dest) else {
            self.notes.push(format!("video-out: cannot use '{dest}'"));
            return;
        };
        let spec = crate::video::VideoSpec {
            format: o.video_format,
            source: crate::video::Source::Native,
            pace: crate::video::Pace::Emulated,
            title: Some(self.title.clone()),
            ..crate::video::VideoSpec::new(parsed)
        };
        let (video, note) = crate::video::VideoOut::open(spec);
        if let Some(n) = note {
            self.notes.push(format!("video-out: {n}"));
        }
        self.notes.push(format!("video-out: {}", video.describe()));
        self.video = video;
    }

    #[cfg(feature = "audio")]
    fn open_audio(&mut self, o: &Options) {
        if !o.audio {
            return;
        }
        use crate::sound::AudioPlayer as _;
        crate::sound::host_status::reset();
        match crate::sound::cpal_audio::CpalPlayer::new() {
            Some((player, stream)) => {
                let rate = player.samples_rate();
                self.gb.enable_audio(Box::new(player));
                self._audio = Some(stream);
                self.notes.push(format!("audio: playing at {rate} Hz"));
            }
            None => self
                .notes
                .push("audio: no output device — running silent".to_string()),
        }
    }

    #[cfg(not(feature = "audio"))]
    fn open_audio(&mut self, o: &Options) {
        if o.audio {
            self.notes.push(
                "audio: this build has no host audio — rebuild with \
                 `--features audio` (needs ALSA dev headers on Linux)"
                    .to_string(),
            );
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn frame_count(&self) -> u64 {
        self.frame
    }

    pub fn rom_path(&self) -> Option<&Path> {
        self.rom_path.as_deref()
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Run one emulated frame, publishing it and honouring the pacing choice.
    fn one_frame(&mut self, speed: Speed) {
        self.gb.frame();
        self.frame += 1;
        if self.video.is_enabled() {
            let now = Instant::now();
            self.video.frame(self.gb.image(), 160, 144, now);
        }
        if let Speed::Realtime(mult) = speed {
            let period = FRAME_PERIOD.div_f64(mult);
            let now = Instant::now();
            if self.next_frame_at <= now {
                // Behind: resync rather than accumulating a debt that would
                // make the next thousand frames run flat out to "catch up".
                self.next_frame_at = now + period;
            } else {
                std::thread::sleep(self.next_frame_at - now);
                self.next_frame_at += period;
            }
        }
    }

    /// Advance `frames` frames. Returns how many actually ran.
    pub fn step(&mut self, frames: u32, o: &Options) -> u32 {
        let n = frames.min(o.max_frames);
        for _ in 0..n {
            self.one_frame(o.speed);
        }
        n
    }

    /// Reboot the cartridge from cold, keeping battery RAM — the same thing the
    /// console's power switch does.
    pub fn reset(&mut self, o: &Options) -> Result<(), String> {
        let path = self
            .rom_path
            .clone()
            .ok_or_else(|| "this cartridge has no path to reload from".to_string())?;
        let data =
            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.gb.flush_battery();
        let save = if o.battery { Some(path.clone()) } else { None };
        self.gb = Gameboy::try_new_with_model(data.clone(), save, o.model)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        // A reset rebuilds the machine, so the declaration has to be reapplied
        // — otherwise `reset` silently turns the picture back on.
        self.gb.set_render_pixels(o.pixels);
        self.exec = Executor::new(Limits::default());
        self.frame = 0;
        self.notes.clear();
        self.attach_plugins(o, &data);
        Ok(())
    }

    // -- observation -------------------------------------------------------

    /// The structured snapshot, in the shape [`terminalgb_plugin_agent`] already
    /// established — so a client that has read `docs/gen1/agent-play.md`
    /// already knows this format.
    pub fn observe_json(&mut self, allow_write: bool) -> String {
        let caps = grant(allow_write);
        let obs = Gen1Observer::new(self.supported);
        let (game, screen) = self
            .gb
            .with_machine(caps, |m| (obs.read(m), Screen::read(m)));
        let mode = classify(&screen, &game);

        let mut w = Writer::new();
        w.begin_obj();
        w.str_field("rom", &self.title);
        w.num_field("frame", self.frame as f64);
        w.str_field("mode", mode.name());
        w.bool_field("in_game", game.in_game);
        if game.in_game {
            w.str_field("player", &game.player_name);
            w.obj_field("position");
            w.num_field("map", game.position.map as f64);
            if !game.map_name.is_empty() {
                w.str_field("map_name", game.map_name);
            }
            w.num_field("x", game.position.x as f64);
            w.num_field("y", game.position.y as f64);
            match game.position.facing {
                Some(f) => w.str_field("facing", f.name()),
                None => w.null_field("facing"),
            }
            match game.map_size {
                Some((mw, mh)) => {
                    w.num_field("map_width", mw as f64);
                    w.num_field("map_height", mh as f64);
                }
                // No map header loaded means the player is not in control, and
                // that is the field that explains why walking does nothing.
                None => w.bool_field("map_loaded", false),
            }
            w.end_obj();
            w.arr_field("exits");
            for e in &game.exits {
                w.begin_obj();
                w.num_field("x", e.x as f64);
                w.num_field("y", e.y as f64);
                w.num_field("to_map", e.to_map as f64);
                let n = terminalgb_plugin_agent::observe::map_name(e.to_map);
                if !n.is_empty() {
                    w.str_field("to", n);
                }
                w.end_obj();
            }
            w.end_arr();
            w.num_field("badges", game.badges.count_ones() as f64);
            w.num_field("money", game.money as f64);
            w.arr_field("party");
            for m in &game.party {
                w.begin_obj();
                w.str_field("name", &m.nickname);
                w.num_field("level", m.level as f64);
                w.num_field("hp", m.hp as f64);
                w.num_field("max_hp", m.max_hp as f64);
                if let Some(s) = m.status {
                    w.str_field("status", s);
                }
                w.end_obj();
            }
            w.end_arr();
            if game.battle != terminalgb_plugin_trainer::gen1::BattleKind::None {
                w.obj_field("battle");
                w.str_field(
                    "kind",
                    match game.battle {
                        terminalgb_plugin_trainer::gen1::BattleKind::Wild => "wild",
                        _ => "trainer",
                    },
                );
                if let Some(op) = &game.opponent {
                    w.obj_field("opponent");
                    w.str_field("name", &op.nickname);
                    w.num_field("level", op.level as f64);
                    w.num_field("hp", op.hp as f64);
                    w.num_field("max_hp", op.max_hp as f64);
                    w.end_obj();
                }
                w.end_obj();
            }
        } else if !self.supported {
            // Say *why* there is no game state rather than leaving a client to
            // conclude the emulator is broken.
            w.str_field(
                "note",
                "structured game state is Pokémon Red/Blue only; for any other \
                 cartridge use screenshot and read_memory",
            );
        }
        w.obj_field("screen");
        // The same contract the agent plugin's snapshot offers, and for the
        // same reason: a decode of artwork that happens to sit in the font
        // range is nonsense, and nonsense in the same field as good data gets
        // the same trust. `trust` here is the **single-read** verdict only —
        // this server answers one call at a time and does not drive the clock
        // between reads, so it cannot judge "still redrawing"; a client that
        // cares should `step` and observe again.
        let trust = screen.trust();
        w.str_field("trust", trust.label());
        w.str_field("kind", screen.kind().name());
        if trust == Trust::Ok {
            w.str_field("text", &screen.compact_text());
            if let Some(sel) = screen.selected() {
                w.str_field("selected", &sel);
            }
        } else {
            w.str_field("text", "");
            w.str_field(
                "note",
                "screen.text was withheld: the tilemap is not showing readable \
                 text right now. Use screenshot, or step a few frames and \
                 observe again.",
            );
        }
        w.bool_field("waiting_for_input", screen.waiting);
        match screen.cursor_row {
            Some(r) => w.num_field("menu_cursor_row", r as f64),
            None => w.null_field("menu_cursor_row"),
        }
        if let Some(c) = screen.cursor_col {
            w.num_field("menu_cursor_col", c as f64);
        }
        w.end_obj();
        if let Some(fault) = self.gb.fault() {
            w.str_field("fault", fault);
        }
        w.end_obj();
        w.finish()
    }

    /// Whether structured Gen 1 state is available for this cartridge.
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    /// The current frame as a PNG.
    pub fn screenshot_png(&mut self, scale: u32) -> Vec<u8> {
        let scale = scale.clamp(1, 6) as u8;
        let mut png = Vec::new();
        if scale == 1 {
            crate::termimage::PngEncoder::new().encode(
                self.gb.image(),
                160,
                144,
                &mut png,
            );
            return png;
        }
        let mut up =
            crate::upscale::Upscaler::new(crate::upscale::Algorithm::Nearest, scale);
        let (w, h) = up.output_size(160, 144);
        let scaled = up.apply(self.gb.image(), 160, 144).to_vec();
        crate::termimage::PngEncoder::new().encode(&scaled, w as u32, h as u32, &mut png);
        png
    }

    /// Both memory accessors go through the capability grant rather than
    /// through `Gameboy::peek_range`/`debug_write` directly, even though the
    /// server owns the `Gameboy` and could reach either.
    ///
    /// **That is the difference between a claim and a fact.** Filtering
    /// `write_memory` out of `tools/list` is what a *client* sees; this is what
    /// makes the write actually impossible. Without `--allow-write-memory` the
    /// poke is refused by [`Machine`] and reported, not silently dropped.
    pub fn read_memory(
        &mut self,
        address: u16,
        len: usize,
        allow_write: bool,
    ) -> Vec<u8> {
        self.gb
            .with_machine(grant(allow_write), |m| m.peek_range(address, len))
    }

    /// Returns how many bytes actually landed, so a refused write is visible
    /// instead of looking like a successful one.
    pub fn write_memory(
        &mut self,
        address: u16,
        bytes: &[u8],
        allow_write: bool,
    ) -> usize {
        self.gb.with_machine(grant(allow_write), |m| {
            bytes
                .iter()
                .enumerate()
                .filter(|(i, b)| m.poke(address.wrapping_add(*i as u16), **b))
                .count()
        })
    }

    // -- action ------------------------------------------------------------

    /// Press buttons with exact frame timing, running the machine while they
    /// are in flight.
    ///
    /// `hold` and `gap` are frames, and they matter: Gen 1's `HandleMenuInput`
    /// delays several frames after accepting a press, so two presses eight
    /// frames apart look exactly like the emulator dropping input
    /// (`docs/gen1/sharp-edges.md`). The default gap here is the one the
    /// executor uses for menus.
    pub fn press(
        &mut self,
        buttons: &[KeypadKey],
        times: u32,
        hold: u16,
        gap: u16,
        o: &Options,
    ) -> u32 {
        let mut frames = 0u32;
        for _ in 0..times {
            for b in buttons {
                self.gb.queue_press(*b, hold, gap);
                // Run until the queue drains, so the call returns with the
                // press *finished* rather than still in flight — a client that
                // reads state immediately afterwards must see the result.
                while !self.gb.input_idle() && frames < o.max_frames {
                    self.one_frame(o.speed);
                    frames += 1;
                }
            }
        }
        frames
    }

    /// Run a plan of high-level actions through [`Executor`], returning one
    /// outcome per action and the frames spent.
    pub fn run_actions(
        &mut self,
        items: &[Json],
        o: &Options,
    ) -> (Vec<Outcome>, Vec<String>, u32, Option<String>) {
        // The plan's own frame cap is the server's, so an executor that runs
        // long reports *why* it stopped instead of being cut off mid-action by
        // the loop below.
        let limits = Limits {
            plan_frames: o.max_frames,
            ..Limits::default()
        };
        let (plan, errors) = plan_from_json(items, &limits);
        if plan.is_empty() {
            return (Vec::new(), errors, 0, None);
        }
        self.exec = Executor::new(limits);
        self.exec.start(plan);
        let mut frames = 0u32;
        let caps = grant(o.allow_write_memory);
        loop {
            // One frame of emulation, then one tick of the plan against the
            // machine as it now stands. `Executor::tick` does nothing while a
            // press is in flight, which is why this costs a boolean most frames.
            self.one_frame(o.speed);
            frames += 1;
            let obs = Gen1Observer::new(self.supported);
            let exec = &mut self.exec;
            let running = self.gb.with_machine(caps, |m| exec.tick(m, &obs));
            if !running || frames >= o.max_frames {
                break;
            }
        }
        if frames >= o.max_frames && self.exec.is_running() {
            let exec = &mut self.exec;
            self.gb.with_machine(caps, |m| {
                exec.abort("the server's frame cap was reached", m)
            });
        }
        let aborted = self.exec.abort_reason().map(|s| s.to_string());
        (self.exec.outcomes().to_vec(), errors, frames, aborted)
    }

    // -- save states -------------------------------------------------------

    pub fn save_state(&mut self, path: &Path) -> Result<usize, String> {
        let bytes = self.gb.save_state();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(path, &bytes)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(bytes.len())
    }

    pub fn load_state(&mut self, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        self.gb
            .load_state(&bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        // A plan in flight refers to a world that no longer exists.
        self.exec = Executor::new(Limits::default());
        Ok(())
    }
}

/// Where a numbered slot lives. Same layout and the same default directory as
/// the terminal frontend, so a state saved from an MCP session loads with `F8`
/// in the TUI and the other way round.
pub fn slot_path(dir: Option<&Path>, title: &str, slot: u32) -> PathBuf {
    let base = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state"))
            })
            .unwrap_or_else(std::env::temp_dir),
    };
    // Same directory the TUI uses, pre-rename fallback and all, so a state
    // saved here loads there. See `screen::config::LEGACY_APP_DIR`.
    let base = match dir {
        Some(_) => base,
        None => crate::screen::config::app_dir_in(&base).join("states"),
    };
    let mut safe: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if safe.trim_matches('_').is_empty() {
        safe = "rom".to_string();
    }
    base.join(format!("{safe}.slot{slot}.gbss"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_memory_is_not_granted_unless_it_was_asked_for() {
        assert!(!grant(false).contains(Capability::WRITE_MEMORY));
        assert!(grant(false).contains(Capability::READ_MEMORY));
        assert!(grant(false).contains(Capability::CONTROL_INPUT));
        assert!(grant(true).contains(Capability::WRITE_MEMORY));
    }

    /// Filtering the tool out of `tools/list` is what a *client* sees; this is
    /// what makes the write impossible. Both matter, and only this one survives
    /// a client that calls the tool anyway.
    #[test]
    fn a_write_without_the_grant_does_not_land_and_says_so() {
        // A minimal ROM-only cartridge, built here so the check needs no file.
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x00;
        let p = std::env::temp_dir().join("gb-mcp-grant-test.gb");
        std::fs::write(&p, &rom).unwrap();

        let mut host = Host::new(Options::default());
        host.load(&p).expect("loads");
        let s = host.need().unwrap();
        let before = s.read_memory(0xC000, 1, false)[0];
        let other = before.wrapping_add(1);

        assert_eq!(s.write_memory(0xC000, &[other], false), 0, "refused");
        assert_eq!(s.read_memory(0xC000, 1, false)[0], before, "nothing landed");

        assert_eq!(s.write_memory(0xC000, &[other], true), 1, "granted");
        assert_eq!(s.read_memory(0xC000, 1, false)[0], other);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_slot_path_is_namespaced_by_cartridge() {
        let dir = PathBuf::from("/tmp/states");
        let a = slot_path(Some(&dir), "POKEMON BLUE", 1);
        let b = slot_path(Some(&dir), "TETRIS", 1);
        assert_ne!(a, b, "two games must not share a slot");
        assert!(a.to_string_lossy().ends_with(".slot1.gbss"), "{a:?}");
        // A title of nothing but padding still produces a usable name.
        let c = slot_path(Some(&dir), "\0\0\0", 2);
        assert!(c.to_string_lossy().contains("rom.slot2"), "{c:?}");
    }

    #[test]
    fn loading_a_file_that_is_not_a_cartridge_is_an_error_not_a_crash() {
        let mut host = Host::new(Options::default());
        let p = std::env::temp_dir().join("gb-mcp-not-a-rom.bin");
        std::fs::write(&p, b"hello").unwrap();
        let e = host.load(&p).unwrap_err();
        assert!(e.contains("too short"), "{e}");
        assert!(host.session.is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn every_tool_says_the_same_thing_when_there_is_no_cartridge() {
        let mut host = Host::new(Options::default());
        let e = host.need().err().unwrap();
        assert!(e.contains("load_rom"), "{e}");
    }
}

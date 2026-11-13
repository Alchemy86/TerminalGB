//! [`PluginHost`] — owns the attached plugins, enforces what they declared, and
//! keeps a broken one from taking the emulator with it.

use crate::capability::Capability;
use crate::event::Event;
use crate::machine::{Bus, Machine};
use crate::plugin::{Plugin, PluginStatus};
use crate::{ApiVersion, API_VERSION};

/// Registration failure. Deliberately small — there is no plugin manager, only
/// the things that can actually go wrong before a plugin has run at all.
#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// A second plugin asked for [`Capability::SERIAL_LINK`]. A cable has two
    /// ends; the emulated console is one of them.
    LinkAlreadyClaimed(String),
    /// Two plugins with the same [`Plugin::name`].
    DuplicateName(String),
    /// The plugin was built against an incompatible version of the plugin API.
    /// Refused rather than loaded, because a plugin that half-fits misbehaves
    /// in ways that look like emulator bugs.
    ApiMismatch {
        plugin: String,
        wanted: ApiVersion,
        host: ApiVersion,
    },
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterError::LinkAlreadyClaimed(n) => {
                write!(f, "the serial link is already driven by plugin '{n}'")
            }
            RegisterError::DuplicateName(n) => {
                write!(f, "a plugin named '{n}' is already attached")
            }
            RegisterError::ApiMismatch {
                plugin,
                wanted,
                host,
            } => write!(
                f,
                "plugin '{plugin}' needs plugin API {wanted}, this build offers {host} \
                 — rebuild the plugin against a matching gameboy-plugin-api"
            ),
        }
    }
}

impl std::error::Error for RegisterError {}

struct Slot {
    plugin: Box<dyn Plugin>,
    capabilities: Capability,
    /// Sticky: set once this plugin has actually written memory.
    has_written: bool,
    /// Sticky: set once this plugin has actually pressed a button.
    has_pressed: bool,
    /// Cleared once [`Plugin::on_attach`] has run. Attachment is completed on
    /// the first dispatch rather than in `register`, because that is the first
    /// moment a [`Machine`] exists to hand it — `register` deliberately does
    /// not borrow the machine, so a frontend can build its plugin set before
    /// running.
    needs_attach: bool,
    /// Set once the plugin has been retired: it receives no further callbacks
    /// and the reason is shown beside its name.
    retired: Option<String>,
}

impl Slot {
    /// The user-visible warning markers, in severity order: `!` = may write
    /// your game and `!!` = has, then `>` = may press the buttons and `>>` =
    /// has. Both are sticky and both live on the *name*, because a frontend
    /// truncates the detail to fit a panel and the warning must survive that.
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
        s
    }

    fn detail(&self) -> String {
        match &self.retired {
            Some(why) => format!("stopped: {why}"),
            None => self.plugin.status(),
        }
    }
}

/// Run `f`, and if it panics, retire the plugin instead of unwinding into the
/// emulator.
///
/// **This is fault isolation, not a sandbox.** It stops a bug in a plugin from
/// killing a session; it does nothing about a plugin that calls
/// `process::exit`, blocks forever, or corrupts a save through capabilities it
/// legitimately holds. The trust model is stated in the crate docs.
///
/// The plugin is retired *because* it panicked: after an unwind its own
/// invariants may be broken, so calling it again is not safe in the ordinary
/// sense of the word, whatever the compiler thinks. `AssertUnwindSafe` is
/// therefore honest here rather than a shortcut — nothing touched by the
/// panicking call is ever used again.
#[cfg(panic = "unwind")]
fn guarded<R>(what: &str, f: impl FnOnce() -> R) -> Result<R, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked".to_string());
            Err(format!("panicked in {what}: {msg}"))
        }
    }
}

/// With `panic = "abort"` there is nothing to catch: the process is gone before
/// this could run. Release builds of the emulator use `panic = 'abort'`, so
/// **the quarantine below protects `cargo run`, tests and any embedder that
/// unwinds, and cannot protect an aborting release build.** Said plainly rather
/// than implied — see `docs/plugins/writing-a-plugin.md` §"Errors".
#[cfg(not(panic = "unwind"))]
fn guarded<R>(_what: &str, f: impl FnOnce() -> R) -> Result<R, String> {
    Ok(f())
}

/// Owns the attached plugins and dispatches to them.
///
/// Empty by default: an emulator with no plugins pays one `is_empty` test per
/// frame and one per serial transfer.
#[derive(Default)]
pub struct PluginHost {
    slots: Vec<Slot>,
    /// Index into `slots` of the plugin driving the link cable, if any.
    link: Option<usize>,
    /// Optional NDJSON event trace — the *emit* face made concrete over a file
    /// rather than a socket. Enabled with `GB_PLUGIN_TRACE=<path>`.
    trace: Option<Trace>,
    frame: u64,
    /// Retirements not yet shown to the user, so a frontend can raise one
    /// toast per fault instead of polling.
    new_faults: Vec<(String, String)>,
}

struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    buf: String,
}

impl PluginHost {
    pub fn new() -> PluginHost {
        PluginHost::default()
    }

    /// Attach a plugin. Fails rather than silently downgrading.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> Result<(), RegisterError> {
        let caps = plugin.capabilities();
        let name = plugin.name().to_string();
        let wanted = plugin.api_version();
        if !wanted.compatible_with(API_VERSION) {
            return Err(RegisterError::ApiMismatch {
                plugin: name,
                wanted,
                host: API_VERSION,
            });
        }
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
            needs_attach: true,
            retired: None,
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

    /// Every attached plugin, in registration order — for a frontend that wants
    /// to enumerate views rather than name a type.
    pub fn plugins(&self) -> impl Iterator<Item = &dyn Plugin> {
        self.slots.iter().map(|s| &*s.plugin)
    }

    /// Open the NDJSON event trace described by `GB_PLUGIN_TRACE`. Returns a
    /// message on failure so the frontend can tell the user rather than
    /// swallowing it.
    pub fn open_trace_from_env(&mut self) -> Option<String> {
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
                retired: s.retired.clone(),
            })
            .collect()
    }

    /// One line for a status bar. **The user must be able to see that a plugin
    /// is attached**, and specifically that one can write memory or press
    /// buttons.
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

    /// Take the retirements that have happened since this was last called, as
    /// `(plugin name, reason)`. A frontend drains this once a frame and raises
    /// a toast — a plugin that stops working must say so.
    pub fn faults(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.new_faults)
    }

    /// Retire a slot: no further callbacks, reason shown beside its name.
    fn retire(&mut self, idx: usize, why: String) {
        if self.slots[idx].retired.is_some() {
            return;
        }
        let name = self.slots[idx].plugin.name().to_string();
        self.slots[idx].retired = Some(why.clone());
        if self.link == Some(idx) {
            // A retired link partner is an unplugged cable, not a partner that
            // answers wrongly.
            self.link = None;
        }
        self.new_faults.push((name, why));
    }

    fn emit(&mut self, event: &Event) {
        if let Some(t) = self.trace.as_mut() {
            use std::io::Write as _;
            t.buf.clear();
            event.write_json(&mut t.buf);
            t.buf.push('\n');
            let _ = t.out.write_all(t.buf.as_bytes());
        }
    }

    /// Run one callback against slot `idx`, folding back what the plugin did
    /// and retiring it if it failed or panicked.
    fn call<R>(
        &mut self,
        idx: usize,
        bus: &mut dyn Bus,
        what: &str,
        f: impl FnOnce(&mut Box<dyn Plugin>, &mut Machine) -> R,
        default: R,
    ) -> R {
        if self.slots[idx].retired.is_some() {
            return default;
        }
        let caps = self.slots[idx].capabilities;
        let plugin = &mut self.slots[idx].plugin;
        let mut machine = Machine::new(bus, caps);
        let outcome = guarded(what, || f(plugin, &mut machine));
        let (wrote, pressed, failed) = (machine.wrote, machine.pressed, machine.failed);
        self.slots[idx].has_written |= wrote;
        self.slots[idx].has_pressed |= pressed;
        match outcome {
            Ok(v) => {
                if let Some(why) = failed {
                    self.retire(idx, why);
                }
                v
            }
            Err(why) => {
                self.retire(idx, why);
                default
            }
        }
    }

    /// Complete a pending attach for slot `idx`, if it has one.
    fn attach_if_needed(&mut self, idx: usize, bus: &mut dyn Bus) {
        if !self.slots[idx].needs_attach {
            return;
        }
        self.slots[idx].needs_attach = false;
        self.call(
            idx,
            bus,
            "on_attach",
            |p, m| p.on_attach(m),
            (),
        );
    }

    fn deliver(&mut self, event: Event, bus: &mut dyn Bus, category: Capability) {
        self.emit(&event);
        for idx in 0..self.slots.len() {
            // Attachment is completed here, on the first dispatch that reaches
            // the plugin, because this is the first point a `Machine` exists.
            self.attach_if_needed(idx, bus);
            if !self.slots[idx].capabilities.contains(category) {
                continue;
            }
            self.call(
                idx,
                bus,
                "on_event",
                |p, m| p.on_event(&event, m),
                (),
            );
        }
    }

    /// A frame completed.
    pub fn on_frame(&mut self, bus: &mut dyn Bus) {
        if self.slots.is_empty() && self.trace.is_none() {
            return;
        }
        self.frame = self.frame.wrapping_add(1);
        let n = self.frame;
        self.deliver(Event::Frame { number: n }, bus, Capability::OBSERVE);
    }

    /// The game started an internally-clocked transfer with `sent` on the wire.
    /// Returns the byte the link partner shifts back, or `None` for "no cable".
    ///
    /// A plugin gets to *be* the other Game Boy. Because the reply is produced
    /// synchronously inside the write that started the transfer, a virtual
    /// partner has no timing budget to miss.
    pub fn on_serial_transfer(&mut self, sent: u8, bus: &mut dyn Bus) -> Option<u8> {
        let idx = self.link?;
        // A game can clock the link before the first frame boundary, so the
        // link partner completes attachment here too.
        self.attach_if_needed(idx, bus);
        let reply = self.call(
            idx,
            bus,
            "on_serial",
            |p, m| p.on_serial(sent, m),
            None,
        );
        // Observers see the completed exchange, including what the partner
        // answered — that is what makes a protocol trace useful.
        if self.trace.is_some() || self.slots.len() > 1 {
            let ev = Event::SerialTransfer {
                sent,
                received: reply.unwrap_or(0xFF),
            };
            self.deliver(ev, bus, Capability::OBSERVE);
        }
        reply
    }

    /// Drop protocol state in every plugin. Called after a save state is
    /// loaded: plugin state is not machine state, so it cannot be restored, and
    /// a stale half-finished exchange would desync the game silently.
    pub fn reset(&mut self) {
        for idx in 0..self.slots.len() {
            if self.slots[idx].retired.is_some() {
                continue;
            }
            let plugin = &mut self.slots[idx].plugin;
            if let Err(why) = guarded("reset", || plugin.reset()) {
                self.retire(idx, why);
            }
        }
    }

    pub fn flush(&mut self) {
        if let Some(t) = self.trace.as_mut() {
            use std::io::Write as _;
            let _ = t.out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Bus;
    use crate::view::PluginView;
    use crate::KeypadKey;

    /// A memory-only stand-in for the emulator, so the host's contract can be
    /// tested without one.
    #[derive(Default)]
    struct FakeBus {
        mem: Vec<u8>,
        rom: Vec<u8>,
        fault: Option<String>,
        pressed: Vec<(KeypadKey, bool)>,
    }

    impl FakeBus {
        fn new() -> FakeBus {
            FakeBus {
                mem: vec![0; 0x10000],
                rom: vec![0; 0x8000],
                ..Default::default()
            }
        }
    }

    impl Bus for FakeBus {
        fn peek(&mut self, a: u16) -> u8 {
            self.mem[a as usize]
        }
        fn poke(&mut self, a: u16, v: u8) {
            self.mem[a as usize] = v;
        }
        fn peek_rom_bank(&mut self, bank: usize, a: u16) -> u8 {
            let off = bank * 0x4000 + (a as usize & 0x3FFF);
            self.rom.get(off).copied().unwrap_or(0xFF)
        }
        fn rom_title(&self) -> String {
            "FAKE".into()
        }
        fn global_checksum(&self) -> u16 {
            0x1234
        }
        fn rom_bank(&self) -> usize {
            1
        }
        fn set_button(&mut self, key: KeypadKey, down: bool) {
            self.pressed.push((key, down));
        }
        fn queue_press(&mut self, key: KeypadKey, _h: u16, _g: u16) {
            self.pressed.push((key, true));
        }
        fn clear_input(&mut self) {
            self.pressed.clear();
        }
        fn input_idle(&self) -> bool {
            self.pressed.is_empty()
        }
        fn frame_rgba(&self) -> &[u8] {
            &[]
        }
        fn report_fault(&mut self, reason: &str) {
            self.fault = Some(reason.to_string());
        }
        fn fault(&self) -> Option<&str> {
            self.fault.as_deref()
        }
    }

    struct Spy {
        name: String,
        caps: Capability,
        events: usize,
        peeked: Option<u8>,
        poke_ok: Option<bool>,
    }

    impl Spy {
        fn new(name: &str, caps: Capability) -> Spy {
            Spy {
                name: name.to_string(),
                caps,
                events: 0,
                peeked: None,
                poke_ok: None,
            }
        }
    }

    impl Plugin for Spy {
        fn name(&self) -> &str {
            &self.name
        }
        fn capabilities(&self) -> Capability {
            self.caps
        }
        fn status(&self) -> String {
            format!("{} events", self.events)
        }
        fn view(&self) -> Option<PluginView> {
            Some(PluginView::new("spy").line(format!("{} events", self.events)))
        }
        fn on_event(&mut self, _e: &Event, m: &mut Machine) {
            self.events += 1;
            self.peeked = m.peek(0xC000);
            self.poke_ok = Some(m.poke(0xC001, 0x42));
        }
        fn on_serial(&mut self, sent: u8, _m: &mut Machine) -> Option<u8> {
            Some(!sent)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn a_plugin_without_read_capability_cannot_peek() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("blind", Capability::OBSERVE)))
            .unwrap();
        let mut bus = FakeBus::new();
        host.on_frame(&mut bus);
        assert_eq!(host.statuses()[0].detail, "1 events");
        assert!(!host.any_wrote_memory());
        assert_eq!(host.get::<Spy>().unwrap().peeked, None);
        assert_eq!(host.get::<Spy>().unwrap().poke_ok, Some(false));
    }

    #[test]
    fn capabilities_are_enforced_not_trusted() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new(
            "reader",
            Capability::OBSERVE | Capability::READ_MEMORY,
        )))
        .unwrap();
        let mut bus = FakeBus::new();
        bus.mem[0xC000] = 0x99;
        host.on_frame(&mut bus);
        assert_eq!(host.get::<Spy>().unwrap().peeked, Some(0x99));
        assert!(!host.any_wrote_memory());
        assert_eq!(bus.mem[0xC001], 0, "the write was refused, not performed");
    }

    #[test]
    fn a_writing_plugin_is_visible_in_the_status_line() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new(
            "writer",
            Capability::OBSERVE | Capability::WRITE_MEMORY,
        )))
        .unwrap();
        assert!(host.status_line().unwrap().starts_with("writer!:"));
        let mut bus = FakeBus::new();
        host.on_frame(&mut bus);
        assert!(host.any_wrote_memory());
        assert!(host.status_line().unwrap().starts_with("writer!!:"));
        assert_eq!(bus.mem[0xC001], 0x42);
    }

    #[test]
    fn only_one_plugin_may_drive_the_link_cable() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("a", Capability::SERIAL_LINK)))
            .unwrap();
        let err = host
            .register(Box::new(Spy::new("b", Capability::SERIAL_LINK)))
            .unwrap_err();
        assert_eq!(err, RegisterError::LinkAlreadyClaimed("a".into()));
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("x", Capability::OBSERVE)))
            .unwrap();
        assert_eq!(
            host.register(Box::new(Spy::new("x", Capability::OBSERVE)))
                .unwrap_err(),
            RegisterError::DuplicateName("x".into())
        );
    }

    #[test]
    fn the_link_partner_answers_the_byte_the_game_sent() {
        let mut host = PluginHost::new();
        host.register(Box::new(Spy::new("link", Capability::SERIAL_LINK)))
            .unwrap();
        let mut bus = FakeBus::new();
        assert_eq!(host.on_serial_transfer(0x01, &mut bus), Some(0xFE));
    }

    #[test]
    fn no_plugins_means_no_link_partner() {
        let mut host = PluginHost::new();
        let mut bus = FakeBus::new();
        assert!(host.is_empty());
        assert_eq!(host.on_serial_transfer(0x01, &mut bus), None);
        assert_eq!(host.status_line(), None);
    }

    /// A plugin built against a contract this host does not offer is refused at
    /// registration with a message naming both versions — never loaded and left
    /// to misbehave.
    #[test]
    fn an_incompatible_plugin_is_refused_with_both_versions_named() {
        struct FromTheFuture;
        impl Plugin for FromTheFuture {
            fn name(&self) -> &str {
                "future"
            }
            fn capabilities(&self) -> Capability {
                Capability::OBSERVE
            }
            fn api_version(&self) -> ApiVersion {
                ApiVersion::new(API_VERSION.major + 1, 0)
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
        let mut host = PluginHost::new();
        let err = host.register(Box::new(FromTheFuture)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("future"), "{msg}");
        assert!(msg.contains(&API_VERSION.to_string()), "{msg}");
        assert!(host.is_empty(), "nothing was attached");
    }

    /// **A failing plugin must not take the emulator down.** Two ways to fail,
    /// one outcome: the plugin is retired, the reason is reported, every other
    /// plugin keeps running, and the machine is untouched.
    #[cfg(panic = "unwind")]
    #[test]
    fn a_panicking_plugin_is_retired_and_the_emulator_survives() {
        struct Exploder(u32);
        impl Plugin for Exploder {
            fn name(&self) -> &str {
                "exploder"
            }
            fn capabilities(&self) -> Capability {
                Capability::OBSERVE
            }
            fn on_event(&mut self, _e: &Event, _m: &mut Machine) {
                self.0 += 1;
                panic!("kaboom on frame {}", self.0);
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let mut host = PluginHost::new();
        host.register(Box::new(Exploder(0))).unwrap();
        host.register(Box::new(Spy::new("survivor", Capability::OBSERVE)))
            .unwrap();
        let mut bus = FakeBus::new();

        // The default hook would print a backtrace straight into a running
        // terminal UI; the host's job is to keep going, not to be quiet, so the
        // test silences it rather than the library doing so behind your back.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        for _ in 0..3 {
            host.on_frame(&mut bus);
        }
        std::panic::set_hook(prev);

        let faults = host.faults();
        assert_eq!(faults.len(), 1, "retired once, not once per frame");
        assert_eq!(faults[0].0, "exploder");
        assert!(faults[0].1.contains("kaboom on frame 1"), "{:?}", faults[0]);

        let st = host.statuses();
        assert!(st[0].retired.is_some());
        assert!(st[0].detail.starts_with("stopped:"), "{:?}", st[0]);
        assert_eq!(
            host.get::<Spy>().unwrap().events,
            3,
            "the other plugin ran every frame"
        );
    }

    /// The graceful half: a plugin that knows it cannot continue says so, and
    /// is retired the same way as one that panicked.
    #[test]
    fn a_plugin_can_retire_itself_and_is_never_called_again() {
        struct GivesUp {
            calls: u32,
        }
        impl Plugin for GivesUp {
            fn name(&self) -> &str {
                "gives-up"
            }
            fn capabilities(&self) -> Capability {
                Capability::OBSERVE
            }
            fn on_event(&mut self, _e: &Event, m: &mut Machine) {
                self.calls += 1;
                m.fail("the table is not where it should be");
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
        let mut host = PluginHost::new();
        host.register(Box::new(GivesUp { calls: 0 })).unwrap();
        let mut bus = FakeBus::new();
        for _ in 0..5 {
            host.on_frame(&mut bus);
        }
        assert_eq!(host.get::<GivesUp>().unwrap().calls, 1);
        let faults = host.faults();
        assert_eq!(faults.len(), 1);
        assert!(faults[0].1.contains("not where it should be"));
        assert!(host.faults().is_empty(), "faults are drained, not repeated");
    }

    /// A retired link partner is an unplugged cable — the game waits, which is
    /// what an absent partner has always looked like — never a partner that
    /// answers wrongly.
    #[cfg(panic = "unwind")]
    #[test]
    fn a_retired_link_partner_becomes_an_unplugged_cable() {
        struct BadCable;
        impl Plugin for BadCable {
            fn name(&self) -> &str {
                "bad-cable"
            }
            fn capabilities(&self) -> Capability {
                Capability::SERIAL_LINK
            }
            fn on_serial(&mut self, _s: u8, _m: &mut Machine) -> Option<u8> {
                panic!("desync");
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
        let mut host = PluginHost::new();
        host.register(Box::new(BadCable)).unwrap();
        let mut bus = FakeBus::new();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        assert_eq!(host.on_serial_transfer(0x01, &mut bus), None);
        std::panic::set_hook(prev);
        assert!(!host.has_link_partner());
        assert_eq!(host.on_serial_transfer(0x01, &mut bus), None);
    }

    #[test]
    fn a_plugin_can_read_a_rom_bank_the_game_has_not_mapped() {
        let mut bus = FakeBus::new();
        bus.rom = vec![0u8; 8 * 0x4000];
        for bank in 0..8usize {
            bus.rom[bank * 0x4000] = 0xA0 + bank as u8;
        }
        let mut m = Machine::new(&mut bus, Capability::READ_MEMORY);
        for bank in 0..8usize {
            assert_eq!(m.peek_bank(bank, 0x4000), Some(0xA0 + bank as u8));
            assert_eq!(m.peek_bank(bank, 0x0000), m.peek_bank(bank, 0x4000));
        }
        assert_eq!(m.peek_bank(999, 0x4000), Some(0xFF), "open bus, not a panic");

        let mut blind = Machine::new(&mut bus, Capability::OBSERVE);
        assert_eq!(
            blind.peek_bank(0, 0x0000),
            None,
            "peek_bank without READ_MEMORY is refused like any other read"
        );
    }
}

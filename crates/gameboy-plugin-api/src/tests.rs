//! Contract tests for the plugin API.
//!
//! These run against a **fake machine** rather than the emulator, which is the
//! point: this crate does not depend on `gameboy`, so the guarantees it makes —
//! capability enforcement, version refusal, panic quarantine — are provable on
//! their own. The tests that need a real CPU (that a plugin's serial reply
//! actually lands in `SB`) live in the emulator, in `src/plugin.rs`.

use super::*;

/// The smallest thing that satisfies [`MachineAccess`]: 64 KiB of flat memory,
/// a few ROM banks, and a record of what was done to it.
///
/// This is also the shape a plugin author copies to unit-test their own plugin
/// with no emulator involved — see `plugins/example-tracer`.
pub struct FakeMachine {
    pub mem: Vec<u8>,
    pub banks: Vec<Vec<u8>>,
    pub buttons: Vec<(KeypadKey, bool)>,
    pub queued: Vec<(KeypadKey, u16, u16)>,
    pub fault: Option<String>,
    pub rgba: Vec<u8>,
}

impl FakeMachine {
    pub fn new() -> FakeMachine {
        FakeMachine {
            mem: vec![0; 0x10000],
            banks: (0..8u8).map(|b| vec![0xA0 + b; 0x4000]).collect(),
            buttons: Vec::new(),
            queued: Vec::new(),
            fault: None,
            rgba: vec![0; 160 * 144 * 4],
        }
    }
}

impl MachineAccess for FakeMachine {
    fn peek(&mut self, address: u16) -> u8 {
        self.mem[address as usize]
    }
    fn poke(&mut self, address: u16, value: u8) {
        self.mem[address as usize] = value;
    }
    fn peek_bank(&mut self, bank: usize, address: u16) -> u8 {
        self.banks
            .get(bank)
            .map(|b| b[address as usize % 0x4000])
            .unwrap_or(0xFF)
    }
    fn rom_title(&self) -> String {
        "FAKE".to_string()
    }
    fn global_checksum(&self) -> u16 {
        0x1234
    }
    fn rom_bank(&self) -> usize {
        1
    }
    fn set_button(&mut self, key: KeypadKey, down: bool) {
        self.buttons.push((key, down));
    }
    fn queue_press(&mut self, key: KeypadKey, hold: u16, gap: u16) {
        self.queued.push((key, hold, gap));
    }
    fn input_idle(&self) -> bool {
        self.queued.is_empty()
    }
    fn clear_input(&mut self) {
        self.queued.clear();
    }
    fn frame_rgba(&self) -> &[u8] {
        &self.rgba
    }
    fn frame_png(&self) -> Vec<u8> {
        b"\x89PNG".to_vec()
    }
    fn report_fault(&mut self, reason: &str) {
        if self.fault.is_none() {
            self.fault = Some(reason.to_string());
        }
    }
    fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
}

/// A plugin that tries everything, so a test can see which attempts the host
/// actually let through.
struct Spy {
    name: String,
    caps: Capability,
    events: usize,
    last_serial: Option<u8>,
    peeked: Option<u8>,
    poke_ok: Option<bool>,
}

impl Spy {
    fn new(name: &str, caps: Capability) -> Spy {
        Spy {
            name: name.to_string(),
            caps,
            events: 0,
            last_serial: None,
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
    fn on_event(&mut self, _e: &Event, m: &mut Machine) {
        self.events += 1;
        self.peeked = m.peek(0xC000);
        self.poke_ok = Some(m.poke(0xC001, 0x42));
    }
    fn on_serial(&mut self, sent: u8, _m: &mut Machine) -> Option<u8> {
        self.last_serial = Some(sent);
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
fn capabilities_are_enforced_not_trusted() {
    let mut host = PluginHost::new();
    host.register(Box::new(Spy::new(
        "reader",
        Capability::OBSERVE | Capability::READ_MEMORY,
    )))
    .unwrap();
    let mut m = FakeMachine::new();
    m.mem[0xC000] = 0x99;
    host.on_frame(&mut m);
    // Read worked, write did not, and the write attempt did not flip the
    // "has written" flag the status bar relies on.
    assert!(!host.any_wrote_memory());
    assert_eq!(m.mem[0xC001], 0, "the poke must not have landed");
}

#[test]
fn a_plugin_without_read_capability_cannot_peek() {
    let mut host = PluginHost::new();
    host.register(Box::new(Spy::new("blind", Capability::OBSERVE)))
        .unwrap();
    let mut m = FakeMachine::new();
    m.mem[0xC000] = 0x99;
    host.on_frame(&mut m);
    assert_eq!(host.statuses()[0].detail, "1 events");
    assert!(!host.any_wrote_memory());
    assert_eq!(m.mem[0xC001], 0);
}

#[test]
fn a_writing_plugin_is_visible_in_the_status_line() {
    let mut host = PluginHost::new();
    host.register(Box::new(Spy::new(
        "writer",
        Capability::OBSERVE | Capability::WRITE_MEMORY,
    )))
    .unwrap();
    // Declared but not yet used: single bang.
    assert!(host.status_line().unwrap().starts_with("writer!:"));
    let mut m = FakeMachine::new();
    host.on_frame(&mut m);
    // Actually wrote: double bang, and it stays.
    assert!(host.any_wrote_memory());
    assert!(host.status_line().unwrap().starts_with("writer!!:"));
    assert_eq!(m.mem[0xC001], 0x42);
}

/// Cartridge tables live in banks the game maps only while it is using them,
/// so an observer must be able to name a bank.
#[test]
fn a_plugin_can_read_a_rom_bank_the_game_has_not_mapped() {
    let mut mem = FakeMachine::new();
    let mut m = Machine::new(&mut mem, Capability::READ_MEMORY);
    for bank in 0..8usize {
        assert_eq!(m.peek_bank(bank, 0x4000), Some(0xA0 + bank as u8));
        // The $0000 and $4000 forms address the same window.
        assert_eq!(m.peek_bank(bank, 0x0000), m.peek_bank(bank, 0x4000));
    }
    // Past the end of the cartridge is open bus, not a panic.
    assert_eq!(m.peek_bank(999, 0x4000), Some(0xFF));

    // It is a read, so it is gated like one.
    let mut blind = Machine::new(&mut mem, Capability::OBSERVE);
    assert_eq!(blind.peek_bank(0, 0x0000), None);
}

/// **A plugin may always stop the emulator writing the save file.**
///
/// Every other entry point on `Machine` is gated, because every other one lets
/// a plugin *do* something to the machine. This one only takes a right away, so
/// the fail-safe direction is to allow it — and it matters, because a
/// game-specific plugin recognises a crash the CPU-level detectors cannot.
#[test]
fn any_plugin_can_stop_the_emulator_writing_the_save() {
    struct Alarm;
    impl Plugin for Alarm {
        fn name(&self) -> &str {
            "alarm"
        }
        fn capabilities(&self) -> Capability {
            // The weakest possible plugin: observe, nothing else.
            Capability::OBSERVE
        }
        fn on_event(&mut self, _e: &Event, m: &mut Machine) {
            m.report_fault("the game has left the rails");
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    let mut mem = FakeMachine::new();
    assert_eq!(mem.fault(), None);
    let mut host = PluginHost::new();
    host.register(Box::new(Alarm)).unwrap();
    host.on_frame(&mut mem);
    assert_eq!(mem.fault(), Some("the game has left the rails"));
}

#[test]
fn button_presses_are_a_declared_capability_and_are_visible() {
    struct Presser(Capability);
    impl Plugin for Presser {
        fn name(&self) -> &str {
            "presser"
        }
        fn capabilities(&self) -> Capability {
            self.0
        }
        fn on_event(&mut self, _e: &Event, m: &mut Machine) {
            m.tap(KeypadKey::A);
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    // Undeclared: refused, nothing queued, no marker.
    let mut host = PluginHost::new();
    host.register(Box::new(Presser(Capability::OBSERVE)))
        .unwrap();
    let mut m = FakeMachine::new();
    host.on_frame(&mut m);
    assert!(m.queued.is_empty());
    assert!(!host.any_pressed_buttons());
    assert_eq!(host.status_line().unwrap(), "presser");

    // Declared: queued, and the name carries the `>>` marker.
    let mut host = PluginHost::new();
    host.register(Box::new(Presser(
        Capability::OBSERVE | Capability::CONTROL_INPUT,
    )))
    .unwrap();
    assert!(
        host.status_line().unwrap().starts_with("presser>"),
        "a plugin that may press buttons says so before it presses one"
    );
    let mut m = FakeMachine::new();
    host.on_frame(&mut m);
    assert_eq!(m.queued, vec![(KeypadKey::A, DEFAULT_HOLD, DEFAULT_GAP)]);
    assert!(host.any_pressed_buttons());
    assert!(host.status_line().unwrap().starts_with("presser>>"));
    assert!(host.statuses()[0].has_pressed);
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
    let mut m = FakeMachine::new();
    assert_eq!(host.on_serial_transfer(0x01, &mut m), Some(0xFE));
}

#[test]
fn no_plugins_means_no_link_partner() {
    let mut host = PluginHost::new();
    let mut m = FakeMachine::new();
    assert!(host.is_empty());
    assert_eq!(host.on_serial_transfer(0x01, &mut m), None);
    assert_eq!(host.status_line(), None);
}

#[test]
fn events_serialise_to_the_documented_ndjson_shape() {
    let mut s = String::new();
    Event::Frame { number: 7 }.write_json(&mut s);
    assert_eq!(s, r#"{"ev":"frame","number":7}"#);
    s.clear();
    Event::SerialTransfer {
        sent: 1,
        received: 2,
    }
    .write_json(&mut s);
    assert_eq!(s, r#"{"ev":"serial","sent":1,"received":2}"#);
}

/// **The version boundary.** A plugin built against a different revision of
/// this contract is refused with an error that says what to do about it, not
/// loaded and left to misbehave.
#[test]
fn a_plugin_from_a_different_api_revision_is_refused() {
    struct FromTheFuture;
    impl Plugin for FromTheFuture {
        fn name(&self) -> &str {
            "future"
        }
        fn capabilities(&self) -> Capability {
            Capability::OBSERVE
        }
        // Only a plugin lying about its version can reach this path in a
        // build-time-linked world, which is exactly what this stands in for.
        fn api_version(&self) -> u32 {
            API_VERSION + 1
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
    assert_eq!(
        err,
        RegisterError::ApiVersionMismatch {
            plugin: API_VERSION + 1,
            host: API_VERSION,
        }
    );
    assert!(host.is_empty(), "a refused plugin must not be attached");
    assert!(err.to_string().contains("rebuild the plugin"));
}

/// **A failing plugin must not take the emulator down.**
///
/// The panicking plugin is quarantined — switched off, marked `x`, reported
/// once — and every other plugin keeps receiving events. Note this test proves
/// the mechanism; under `panic = 'abort'` (this repo's release profile) no
/// `catch_unwind` in Rust can catch anything, which `docs/plugins.md` states
/// plainly. Cargo builds test binaries with unwinding regardless of the
/// profile, which is why this runs under `cargo test --release` too.
#[test]
fn a_panicking_plugin_is_quarantined_and_the_rest_keep_running() {
    struct Exploder;
    impl Plugin for Exploder {
        fn name(&self) -> &str {
            "exploder"
        }
        fn capabilities(&self) -> Capability {
            Capability::OBSERVE
        }
        fn on_event(&mut self, _e: &Event, _m: &mut Machine) {
            panic!("bad cartridge assumption");
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    // Silence the panic hook's backtrace: the panic here is the test's subject,
    // not a failure, and its message on stderr reads like one.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut host = PluginHost::new();
    host.register(Box::new(Exploder)).unwrap();
    host.register(Box::new(Spy::new("survivor", Capability::OBSERVE)))
        .unwrap();
    let mut m = FakeMachine::new();

    for _ in 0..5 {
        host.on_frame(&mut m);
    }
    std::panic::set_hook(previous);

    let st = host.statuses();
    assert!(st[0].failed, "the panicking plugin is marked failed");
    assert_eq!(st[0].name, "exploderx", "and the user can see it");
    assert_eq!(st[0].detail, "stopped after an internal error");
    assert!(!st[1].failed, "its neighbour is untouched");
    assert_eq!(
        st[1].detail, "5 events",
        "every frame still reached the surviving plugin"
    );

    // Reported exactly once, however many frames ran, so a toast is not a storm.
    let failures = host.take_failures();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("exploder"));
    assert!(failures[0].contains("bad cartridge assumption"));
    assert!(host.take_failures().is_empty(), "drained, not repeated");
}

/// A link partner that panics leaves the cable unplugged rather than wedging
/// the transfer: the game reads `$FF` and carries on, which is a state it
/// already handles.
#[test]
fn a_panicking_link_partner_becomes_an_unplugged_cable() {
    struct BadLink;
    impl Plugin for BadLink {
        fn name(&self) -> &str {
            "bad-link"
        }
        fn capabilities(&self) -> Capability {
            Capability::SERIAL_LINK
        }
        fn on_serial(&mut self, _sent: u8, _m: &mut Machine) -> Option<u8> {
            panic!("protocol desync");
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut host = PluginHost::new();
    host.register(Box::new(BadLink)).unwrap();
    let mut m = FakeMachine::new();
    assert_eq!(host.on_serial_transfer(0x01, &mut m), None);
    // And it stays unplugged rather than panicking once per byte.
    assert_eq!(host.on_serial_transfer(0x02, &mut m), None);
    std::panic::set_hook(previous);
    assert_eq!(host.take_failures().len(), 1);
}

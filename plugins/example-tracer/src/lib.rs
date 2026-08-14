//! **A complete, working plugin in one file. Copy this directory to start.**
//!
//! It watches one address in the emulated machine and reports when the byte
//! there changes — the first plugin most people actually want, because "what
//! writes this?" is the first question you ask about a game you are reverse
//! engineering.
//!
//! Everything a plugin needs is here and nothing else is:
//!
//! * it declares its capabilities ([`Plugin::capabilities`]) and the host
//!   enforces them;
//! * it reacts to an event and reads memory in response ([`Plugin::on_event`]);
//! * it reports one line for the status bar ([`Plugin::status`]);
//! * it drops its state when a save state is loaded ([`Plugin::reset`]);
//! * it is tested with no emulator at all, against a fake machine.
//!
//! See [`docs/plugins.md`](https://github.com/Alchemy86/gameboy/blob/main/docs/plugins.md)
//! for how to install it.

use gameboy_plugin_api::{Capability, Event, Machine, Plugin};

/// The name the user puts in `plugins = …`. Keep it short and stable: it is
/// configuration, and it shows up in the status bar.
pub const NAME: &str = "example-tracer";

/// How many changes to remember. A plugin runs inside the emulated frame, so
/// unbounded growth is a leak with a 60 Hz clock on it.
const HISTORY: usize = 16;

/// One observed change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    /// The frame it was noticed on.
    pub frame: u64,
    pub from: u8,
    pub to: u8,
}

/// Watches one address and records what it changes to.
pub struct Tracer {
    address: u16,
    last: Option<u8>,
    frame: u64,
    changes: Vec<Change>,
}

impl Tracer {
    pub fn new(address: u16) -> Tracer {
        Tracer {
            address,
            last: None,
            frame: 0,
            changes: Vec::new(),
        }
    }

    /// Everything seen so far, oldest first. A plugin never draws — it exposes
    /// state, and the frontend decides what to do with it. See
    /// `docs/plugins.md` §"A plugin never draws".
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn address(&self) -> u16 {
        self.address
    }
}

impl Plugin for Tracer {
    fn name(&self) -> &str {
        NAME
    }

    /// Ask for the least that will do the job.
    ///
    /// `OBSERVE` to be told about frames, `READ_MEMORY` to look at one. This
    /// plugin can never write the game or press a button, and that is enforced
    /// by the host rather than promised by this comment: `Machine::poke` would
    /// return `false` without doing anything. See the test below.
    fn capabilities(&self) -> Capability {
        Capability::OBSERVE | Capability::READ_MEMORY
    }

    /// One line for the status bar. Called at most once per *displayed* frame,
    /// so it may allocate, but it should stay cheap and short.
    fn status(&self) -> String {
        match self.changes.last() {
            None => format!("watching ${:04X}", self.address),
            Some(c) => format!(
                "${:04X}: {:02X}->{:02X} ({} changes)",
                self.address,
                c.from,
                c.to,
                self.changes.len()
            ),
        }
    }

    /// The whole plugin. `machine` comes with the event precisely so a plugin
    /// can read state *in response to* it.
    fn on_event(&mut self, event: &Event, machine: &mut Machine) {
        // `Event` is `#[non_exhaustive]`, so match the ones you care about and
        // ignore the rest. A host built after your plugin may send categories
        // you have never heard of; that must not be an error.
        let Event::Frame { number } = event else {
            return;
        };
        self.frame = *number;

        // `peek` returns `None` if this plugin had not declared READ_MEMORY.
        // Handling that rather than unwrapping is the difference between a
        // plugin that degrades and one that panics.
        let Some(value) = machine.peek(self.address) else {
            return;
        };

        match self.last {
            Some(previous) if previous != value => {
                if self.changes.len() == HISTORY {
                    self.changes.remove(0);
                }
                self.changes.push(Change {
                    frame: self.frame,
                    from: previous,
                    to: value,
                });
            }
            _ => {}
        }
        self.last = Some(value);
    }

    /// Plugin state is not machine state and is not serialised, so a loaded
    /// save state must not leave stale observations behind.
    fn reset(&mut self) {
        self.last = None;
        self.changes.clear();
    }

    // Required boilerplate: it is how a frontend recovers your concrete type in
    // order to render it. Copy these two verbatim.
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gameboy_plugin_api::{KeypadKey, MachineAccess, PluginHost};

    /// **A plugin is testable without an emulator.** Implement `MachineAccess`
    /// over whatever you like — here, a flat array — and drive `PluginHost`
    /// yourself. No ROM, no CPU, no display.
    ///
    /// When a plugin genuinely needs a real cartridge (banked ROM tables, an
    /// MBC), use `gameboy::testing::TestMachine` as a **dev**-dependency
    /// instead; `plugins/battle-control` does exactly that.
    struct FakeMachine {
        mem: [u8; 0x10000],
    }

    impl FakeMachine {
        fn new() -> FakeMachine {
            FakeMachine { mem: [0; 0x10000] }
        }
    }

    impl MachineAccess for FakeMachine {
        fn peek(&mut self, address: u16) -> u8 {
            self.mem[address as usize]
        }
        fn poke(&mut self, address: u16, value: u8) {
            self.mem[address as usize] = value;
        }
        fn peek_bank(&mut self, _bank: usize, _address: u16) -> u8 {
            0xFF
        }
        fn rom_title(&self) -> String {
            "TEST".into()
        }
        fn global_checksum(&self) -> u16 {
            0
        }
        fn rom_bank(&self) -> usize {
            1
        }
        fn set_button(&mut self, _key: KeypadKey, _down: bool) {}
        fn queue_press(&mut self, _key: KeypadKey, _hold: u16, _gap: u16) {}
        fn input_idle(&self) -> bool {
            true
        }
        fn clear_input(&mut self) {}
        fn frame_rgba(&self) -> &[u8] {
            &[]
        }
        fn frame_png(&self) -> Vec<u8> {
            Vec::new()
        }
        fn report_fault(&mut self, _reason: &str) {}
        fn fault(&self) -> Option<&str> {
            None
        }
    }

    #[test]
    fn it_reports_a_change_and_not_a_steady_value() {
        let mut host = PluginHost::new();
        host.register(Box::new(Tracer::new(0xC123))).unwrap();
        let mut m = FakeMachine::new();

        // Three frames with the value steady: nothing to report.
        m.mem[0xC123] = 0x10;
        for _ in 0..3 {
            host.on_frame(&mut m);
        }
        assert_eq!(host.statuses()[0].detail, "watching $C123");

        // It changes: reported once, with the frame it happened on.
        m.mem[0xC123] = 0x2A;
        host.on_frame(&mut m);
        let t = host.get::<Tracer>().unwrap();
        assert_eq!(
            t.changes(),
            &[Change {
                frame: 4,
                from: 0x10,
                to: 0x2A
            }]
        );
        assert_eq!(host.statuses()[0].detail, "$C123: 10->2A (1 changes)");
    }

    /// The capability model is enforced by the host, not by this plugin's good
    /// manners: a write is refused because `WRITE_MEMORY` was never declared.
    #[test]
    fn this_plugin_cannot_write_even_if_it_tries() {
        struct Rogue;
        impl Plugin for Rogue {
            fn name(&self) -> &str {
                "rogue"
            }
            // The same capabilities Tracer asks for.
            fn capabilities(&self) -> Capability {
                Capability::OBSERVE | Capability::READ_MEMORY
            }
            fn on_event(&mut self, _e: &Event, m: &mut Machine) {
                assert!(!m.poke(0xC000, 0xFF), "the host must refuse this");
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let mut host = PluginHost::new();
        host.register(Box::new(Rogue)).unwrap();
        let mut m = FakeMachine::new();
        host.on_frame(&mut m);
        assert_eq!(m.mem[0xC000], 0, "memory is untouched");
        assert!(!host.any_wrote_memory(), "and the status bar agrees");
    }

    /// Loading a save state resets plugins, because plugin state is not part of
    /// the machine state.
    #[test]
    fn a_save_state_load_clears_what_was_observed() {
        let mut host = PluginHost::new();
        host.register(Box::new(Tracer::new(0xC000))).unwrap();
        let mut m = FakeMachine::new();
        host.on_frame(&mut m);
        m.mem[0xC000] = 0x99;
        host.on_frame(&mut m);
        assert_eq!(host.get::<Tracer>().unwrap().changes().len(), 1);

        host.reset();
        assert!(host.get::<Tracer>().unwrap().changes().is_empty());
    }
}

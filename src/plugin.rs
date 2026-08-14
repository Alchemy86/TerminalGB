//! Plugins — the *attach* half of the public extension surface.
//!
//! **The contract itself lives in the [`terminalgb-plugin-api`](terminalgb_plugin_api)
//! crate**, which has no dependencies and, crucially, does not depend on this
//! one. This module is the emulator's *side* of that contract: it re-exports the
//! whole API (so `terminalgb::plugin::Plugin` still names the trait, as it always
//! has) and implements [`MachineAccess`] for the memory unit, which is the one
//! thing only the emulator can provide.
//!
//! Why the API is a separate crate rather than a module here: this crate's
//! terminal frontend *draws panels* for the plugins it ships with, so `terminalgb`
//! depends on those plugin crates. If plugins also depended on `terminalgb` for the
//! trait, the dependency graph would be a cycle and Cargo would refuse it. An
//! API crate that neither side owns breaks the knot, and has the pleasant side
//! effect that a plugin author's dependency is 400 lines with no build
//! requirements instead of a whole emulator. See
//! [`docs/plugins.md`](../docs/plugins.md).
//!
//! ```text
//!            terminalgb-plugin-api          (the contract; no dependencies)
//!             ^                ^
//!             |                |
//!    terminalgb (emulator)   terminalgb-plugin-*   (trainer, gen1-link, …)
//!             ^                ^
//!             |                |
//!             +--- optional ---+          (the frontend draws their panels)
//! ```
//!
//! See [`docs/debugging.md`](../docs/debugging.md) for the three-faced extension
//! surface this is the *attach* face of, and [`crate::telemetry`] for the
//! *emit* face.

use crate::mmu::MemoryManagementUnit;

pub use terminalgb_plugin_api::{
    Capability, Event, Machine, MachineAccess, Plugin, PluginHost, PluginStatus,
    RegisterError, API_VERSION,
};

/// The emulator's implementation of the plugin API's machine seam.
///
/// Everything here is ungated — refusal is
/// [`Machine`](terminalgb_plugin_api::Machine)'s job — and nothing here ticks the
/// bus, so a plugin observing memory cannot perturb the run it is watching.
impl MachineAccess for MemoryManagementUnit<'_> {
    fn peek(&mut self, address: u16) -> u8 {
        self.debug_peek(address)
    }

    fn poke(&mut self, address: u16, value: u8) {
        self.debug_poke(address, value);
    }

    fn peek_bank(&mut self, bank: usize, address: u16) -> u8 {
        self.mbc.readrom_bank(bank, address)
    }

    fn rom_title(&self) -> String {
        self.mbc.romname()
    }

    fn global_checksum(&self) -> u16 {
        self.mbc.global_checksum()
    }

    fn rom_bank(&self) -> usize {
        self.mbc.rom_bank()
    }

    fn set_button(&mut self, key: crate::input::KeypadKey, down: bool) {
        if down {
            self.keypad.keydown(key);
        } else {
            self.keypad.keyup(key);
        }
    }

    fn queue_press(&mut self, key: crate::input::KeypadKey, hold: u16, gap: u16) {
        self.input_queue.push(key, hold, gap);
    }

    fn input_idle(&self) -> bool {
        self.input_queue.is_idle()
    }

    fn clear_input(&mut self) {
        let MemoryManagementUnit {
            input_queue,
            keypad,
            ..
        } = self;
        input_queue.reset(keypad);
    }

    fn frame_rgba(&self) -> &[u8] {
        &self.gpu.data[..]
    }

    fn frame_png(&self) -> Vec<u8> {
        // The crate's own encoder: no `image` dependency, and it compresses,
        // unlike the stored-deflate path the 60 Hz wire uses.
        let mut png = Vec::new();
        crate::termimage::PngEncoder::new().encode(
            &self.gpu.data[..],
            160,
            144,
            &mut png,
        );
        png
    }

    fn report_fault(&mut self, reason: &str) {
        MemoryManagementUnit::report_fault(self, reason);
    }

    fn fault(&self) -> Option<&str> {
        MemoryManagementUnit::fault(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam the API crate's own tests cannot reach: that a plugin's serial
    /// reply travels through `Serial::pending`, `Cpu::do_cycle` and
    /// `MMU::do_cycle` in the right order and lands in `SB`.
    ///
    /// Everything else about the contract — capability enforcement, the version
    /// boundary, panic quarantine — is proved against a fake machine in
    /// `terminalgb-plugin-api`, which is the point of that crate standing alone.
    #[test]
    fn a_plugin_answers_a_transfer_the_running_program_started() {
        struct Answer;
        impl Plugin for Answer {
            fn name(&self) -> &str {
                "answer"
            }
            fn capabilities(&self) -> Capability {
                Capability::SERIAL_LINK
            }
            fn on_serial(&mut self, sent: u8, _m: &mut Machine) -> Option<u8> {
                // The Gen 1 handshake in miniature: reply $02 to $01.
                Some(if sent == 0x01 { 0x02 } else { 0xEE })
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }

        let mut cpu = crate::cpu::core::Cpu::new(link_poll_rom(), None);
        cpu.plugins.register(Box::new(Answer)).unwrap();
        cpu.registers.pc = 0x100;
        for _ in 0..8 {
            cpu.do_cycle();
        }
        assert_eq!(
            cpu.memory.debug_peek(0xC000),
            0x01,
            "a transfer is not instantaneous: SB still holds the byte we sent"
        );
        for _ in 0..2000 {
            cpu.do_cycle();
        }
        assert_eq!(
            cpu.memory.debug_peek(0xC000),
            0x02,
            "the plugin's reply should land in SB when the shift clock runs out"
        );
        assert_ne!(
            cpu.memory.intf & 0x08,
            0,
            "completing the transfer requests the serial interrupt"
        );
    }

    /// With nothing attached, the same program sees an unplugged cable: the
    /// console is driving the clock, so the transfer still runs to completion —
    /// it just shifts in the idle-high `$FF` the empty link port presents.
    #[test]
    fn with_no_plugin_a_transfer_shifts_in_an_idle_cable() {
        let mut cpu = crate::cpu::core::Cpu::new(link_poll_rom(), None);
        cpu.registers.pc = 0x100;
        for _ in 0..8 {
            cpu.do_cycle();
        }
        assert_eq!(cpu.memory.debug_peek(0xC000), 0x01);
        assert_eq!(
            cpu.memory.intf & 0x08,
            0,
            "nothing has been shifted yet, so no serial interrupt yet"
        );
        for _ in 0..2000 {
            cpu.do_cycle();
        }
        assert_eq!(
            cpu.memory.debug_peek(0xC000),
            0xFF,
            "an unplugged cable reads as all ones"
        );
        assert_ne!(cpu.memory.intf & 0x08, 0);
    }

    /// A scheduled press has to reach the emulated joypad register — the seam
    /// between the API's `Machine::press` and this crate's `InputQueue`.
    #[test]
    fn a_scheduled_press_reaches_the_joypad_register() {
        struct TapOnce(bool);
        impl Plugin for TapOnce {
            fn name(&self) -> &str {
                "tap"
            }
            fn capabilities(&self) -> Capability {
                Capability::OBSERVE | Capability::CONTROL_INPUT
            }
            fn on_event(&mut self, _e: &Event, m: &mut Machine) {
                if !self.0 {
                    self.0 = true;
                    m.press(crate::input::KeypadKey::Start, 4, 4);
                }
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
        let mut host = PluginHost::new();
        host.register(Box::new(TapOnce(false))).unwrap();
        let mut m = MemoryManagementUnit::new(vec![0u8; 0x8000], None).unwrap();
        host.on_frame(&mut m);
        // Nothing is held until the host advances the queue for a frame.
        m.keypad.wb(0x10);
        assert_eq!(m.keypad.rb() & 0x08, 0x08);
        m.advance_input();
        m.keypad.wb(0x10);
        assert_eq!(m.keypad.rb() & 0x08, 0, "Start reads low while held");
        for _ in 0..4 {
            m.advance_input();
        }
        m.keypad.wb(0x10);
        assert_eq!(m.keypad.rb() & 0x08, 0x08, "and is released again");
    }

    /// Start one transfer and then poll `SB` into `$C000` forever, so a test
    /// can watch the shift register run out instead of assuming it already has.
    fn link_poll_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x112].copy_from_slice(&[
            0x3E, 0x01, // LD A, $01
            0xEA, 0x01, 0xFF, // LD ($FF01), A   ; SB = the byte we send
            0x3E, 0x81, // LD A, $81
            0xEA, 0x02, 0xFF, // LD ($FF02), A   ; SC = start, internal clock
            0xFA, 0x01, 0xFF, // LD A, ($FF01)   ; read the received byte
            0xEA, 0x00, 0xC0, // LD ($C000), A   ; stash it where we can see it
            0x18, 0xF8, // JR -8            ; ...and keep watching
        ]);
        rom
    }

    /// A plugin's fault report must reach the *battery*, or the save is still
    /// at risk. That wiring is the emulator's, so it is proved here.
    #[test]
    fn a_plugin_fault_report_poisons_the_battery() {
        let mut mem = MemoryManagementUnit::new(vec![0u8; 0x8000], None).unwrap();
        assert_eq!(MachineAccess::fault(&mem), None);
        MachineAccess::report_fault(&mut mem, "the game has left the rails");
        assert_eq!(
            mem.mbc.battery_poisoned(),
            Some("the game has left the rails")
        );
    }
}

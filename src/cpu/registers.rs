use crate::mode::{GbMode, Model};
use std::fmt;

#[derive(Debug)]
pub struct Registers {
    // 8-bit registers
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,

    // The flags register (F)
    // it automatically calculates certain bits, or flags, based on the result of the last operation.
    pub f: u8,

    // 16-bit registers
    pub pc: u16,
    pub sp: u16,
}

#[derive(Copy, Clone)]
pub enum CpuFlag {
    C = 0b00010000,
    H = 0b00100000,
    N = 0b01000000,
    Z = 0b10000000,
}

impl fmt::Display for Registers {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "a:{:2x} f:{:2x} b:{:2x} c:{:2x} d:{:2x} e:{:2x} \
                h:{:2x} l:{:2x} pc:{:4x} sp:{:4x}",
            self.a,
            self.b,
            self.c,
            self.d,
            self.e,
            self.f,
            self.h,
            self.l,
            self.pc,
            self.sp
        )
    }
}

impl Registers {
    /// The registers a boot ROM leaves behind at `PC=$0100`, for the console
    /// `model` running in `mode`.
    ///
    /// **Both arguments are load-bearing.** `mode` says what the *cartridge*
    /// sees (a Color console runs a DMG-only game in compatibility mode, which
    /// hands over a different `DE`/`HL` from a Color game); `model` says which
    /// console it is, and consoles that share a `mode` still differ — a DMG0, a
    /// DMG and an MGB are all [`GbMode::Classic`] and hand over three different
    /// states.
    ///
    /// Every row below is one of Mooneye's `boot_regs-*` ROMs, each of which
    /// asserts all eight registers plus `SP` and is verified against the real
    /// console it names. They are mutually exclusive by construction: passing
    /// one is failing the other six.
    pub fn boot(model: Model, mode: GbMode) -> Registers {
        use CpuFlag::*;
        let (a, f, b, c, d, e, h, l) = match model {
            // `boot_regs-dmg0`. The first DMG's boot ROM does not run the
            // header-checksum lock-up that the later revisions end with, so it
            // arrives with whatever the logo comparison left in the registers —
            // which is a completely different set, flags included.
            Model::Dmg0 => (0x01, 0x00, 0xFF, 0x13, 0x00, 0xC1, 0x84, 0x03),
            // `boot_regs-mgb`: the Game Boy Pocket differs from a DMG in `A`
            // alone, and that is how a cartridge detects it.
            Model::Mgb => (
                0xFF,
                C as u8 | H as u8 | Z as u8,
                0x00,
                0x13,
                0x00,
                0xD8,
                0x01,
                0x4D,
            ),
            // `boot_regs-dmgABC`.
            Model::Dmg => (
                0x01,
                C as u8 | H as u8 | Z as u8,
                0x00,
                0x13,
                0x00,
                0xD8,
                0x01,
                0x4D,
            ),
            // The Super Game Boy's boot code leaves a different state again —
            // notably `F` clear rather than `Z H C`, and `HL = $C060`. The two
            // revisions differ only in `A`; Mooneye's `boot_regs-sgb` and
            // `boot_regs-sgb2` are the tests that tell them apart. (Pan Docs,
            // "Console state after boot ROM".)
            Model::Sgb => (0x01, 0x00, 0x00, 0x14, 0x00, 0x00, 0xC0, 0x60),
            Model::Sgb2 => (0xFF, 0x00, 0x00, 0x14, 0x00, 0x00, 0xC0, 0x60),
            // A Game Boy Advance ends its boot ROM with an `inc b`, which is
            // both why `B` holds `$01` — the bit an AGB-aware cartridge tests —
            // and why `F` carries that instruction's result (`$00`) instead of
            // the CGB's `Z`. `misc/boot_regs-A`.
            Model::Agb => match mode {
                GbMode::Color => (0x11, 0x00, 0x01, 0x00, 0xFF, 0x56, 0x00, 0x0D),
                _ => (0x11, 0x00, 0x01, 0x00, 0x00, 0x08, 0x00, 0x7C),
            },
            // Every Color console revision hands over the same registers —
            // `misc/boot_regs-cgb` names no revision, unlike `boot_div`, which
            // splits CGB 0 from CGB A-E.
            _ => match mode {
                GbMode::Color => (0x11, Z as u8, 0x00, 0x00, 0xFF, 0x56, 0x00, 0x0D),
                _ => (0x11, Z as u8, 0x00, 0x00, 0x00, 0x08, 0x00, 0x7C),
            },
        };
        Registers {
            a,
            f,
            b,
            c,
            d,
            e,
            h,
            l,
            pc: 0x0100,
            sp: 0xFFFE,
        }
    }

    pub fn flag(&mut self, flags: CpuFlag, set: bool) {
        let mask = flags as u8;
        match set {
            true => self.f |= mask,
            false => self.f &= !mask,
        }
        self.f &= 0xF0;
    }

    pub fn getflag(&self, flags: CpuFlag) -> bool {
        let mask = flags as u8;
        self.f & mask > 0
    }

    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.a);
        w.u8(self.b);
        w.u8(self.c);
        w.u8(self.d);
        w.u8(self.e);
        w.u8(self.h);
        w.u8(self.l);
        w.u8(self.f);
        w.u16(self.pc);
        w.u16(self.sp);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.a = r.u8()?;
        self.b = r.u8()?;
        self.c = r.u8()?;
        self.d = r.u8()?;
        self.e = r.u8()?;
        self.h = r.u8()?;
        self.l = r.u8()?;
        self.f = r.u8()?;
        self.pc = r.u16()?;
        self.sp = r.u16()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(console, mode, [a, f, b, c, d, e, h, l])` — Mooneye's `boot_regs-*`
    /// tables, transcribed. Each ROM asserts all eight registers plus `SP` and
    /// is verified against the console it names; the suite ships one per
    /// machine because **no two agree**, so this table is also the proof that
    /// the machines are distinguishable at all.
    const BOOT_REGS: &[(Model, GbMode, [u8; 8])] = &[
        // acceptance/boot_regs-dmgABC
        (
            Model::Dmg,
            GbMode::Classic,
            [0x01, 0xB0, 0x00, 0x13, 0x00, 0xD8, 0x01, 0x4D],
        ),
        // acceptance/boot_regs-dmg0
        (
            Model::Dmg0,
            GbMode::Classic,
            [0x01, 0x00, 0xFF, 0x13, 0x00, 0xC1, 0x84, 0x03],
        ),
        // acceptance/boot_regs-mgb
        (
            Model::Mgb,
            GbMode::Classic,
            [0xFF, 0xB0, 0x00, 0x13, 0x00, 0xD8, 0x01, 0x4D],
        ),
        // acceptance/boot_regs-sgb and -sgb2
        (
            Model::Sgb,
            GbMode::Super,
            [0x01, 0x00, 0x00, 0x14, 0x00, 0x00, 0xC0, 0x60],
        ),
        (
            Model::Sgb2,
            GbMode::Super2,
            [0xFF, 0x00, 0x00, 0x14, 0x00, 0x00, 0xC0, 0x60],
        ),
        // misc/boot_regs-cgb — a DMG cartridge, so compatibility mode. The
        // revision makes no difference here; that ROM names none, unlike
        // `boot_div`, which splits CGB 0 from CGB A-E.
        (
            Model::Cgb,
            GbMode::ColorAsClassic,
            [0x11, 0x80, 0x00, 0x00, 0x00, 0x08, 0x00, 0x7C],
        ),
        (
            Model::Cgb0,
            GbMode::ColorAsClassic,
            [0x11, 0x80, 0x00, 0x00, 0x00, 0x08, 0x00, 0x7C],
        ),
        // misc/boot_regs-A — `B = $01` from the AGB boot ROM's closing `inc b`,
        // and `F` is that instruction's result rather than the CGB's `Z`.
        (
            Model::Agb,
            GbMode::ColorAsClassic,
            [0x11, 0x00, 0x01, 0x00, 0x00, 0x08, 0x00, 0x7C],
        ),
    ];

    fn boot(model: Model, mode: GbMode) -> [u8; 8] {
        let r = Registers::boot(model, mode);
        [r.a, r.f, r.b, r.c, r.d, r.e, r.h, r.l]
    }

    #[test]
    fn every_console_hands_over_the_registers_its_own_rom_asserts() {
        for &(model, mode, want) in BOOT_REGS {
            assert_eq!(boot(model, mode), want, "{model:?}/{mode:?}");
            // Every `boot_regs` ROM checks `SP` first and quits with
            // "INVALID SP VALUE" if it is wrong.
            let r = Registers::boot(model, mode);
            assert_eq!((r.sp, r.pc), (0xFFFE, 0x0100), "{model:?}");
        }
    }

    /// The point of shipping one ROM per machine: a state that satisfies two of
    /// them would mean the emulator cannot tell those consoles apart.
    #[test]
    fn no_two_consoles_hand_over_the_same_registers() {
        for (i, &(a, am, _)) in BOOT_REGS.iter().enumerate() {
            for &(b, bm, _) in &BOOT_REGS[i + 1..] {
                // CGB and CGB 0 genuinely agree — `boot_regs-cgb` covers both,
                // and only `boot_div` separates them.
                if matches!((a, b), (Model::Cgb, Model::Cgb0)) {
                    continue;
                }
                assert_ne!(
                    boot(a, am),
                    boot(b, bm),
                    "{a:?} and {b:?} are indistinguishable"
                );
            }
        }
    }

    /// A Color console runs a CGB-aware cartridge through a different path in
    /// its boot ROM and hands over a different `DE`/`HL` — the two are not the
    /// same machine, which is what `Color` vs `ColorAsClassic` is for.
    #[test]
    fn a_colour_cartridge_and_a_compatibility_cartridge_differ() {
        let colour = Registers::boot(Model::Cgb, GbMode::Color);
        let compat = Registers::boot(Model::Cgb, GbMode::ColorAsClassic);
        assert_eq!(
            (colour.d, colour.e, colour.h, colour.l),
            (0xFF, 0x56, 0x00, 0x0D)
        );
        assert_eq!(
            (compat.d, compat.e, compat.h, compat.l),
            (0x00, 0x08, 0x00, 0x7C)
        );
        assert_eq!(colour.a, compat.a);
    }
}

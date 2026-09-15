//! An **optional, user-supplied boot ROM**.
//!
//! This emulator's default is high-level emulation of the boot sequence: no
//! boot ROM runs, and `MemoryManagementUnit::set_initial` writes by hand the
//! state one would have left behind at `PC=$0100` (see
//! [`docs/mealybug.md`](../docs/mealybug.md) §8.6 for what that costs). The
//! registers, the system counter and the APU are transcribed from Mooneye's
//! hardware-verified tables and are exact. **Video RAM is not**, and cannot be:
//! the tile the boot ROM leaves at `$8190` is a glyph — bitmap artwork out of
//! somebody else's ROM, which is a different kind of thing from a register
//! value, and not ours to redistribute at any size. Ten Mealybug ROMs draw with
//! that tile and nothing else.
//!
//! So the accurate route is the one every other emulator takes: **let the user
//! supply a boot ROM they dumped from hardware they own.** When one is present
//! it is mapped over the cartridge and executed, exactly as the console does,
//! and the handoff state is real rather than reconstructed. When one is absent
//! — which is the normal case, and the case every test and every published
//! score in this repository is measured in — nothing changes.
//!
//! Three rules, and they are not negotiable:
//!
//! * **Never bundle one.** Nothing in `third_party/`, nothing in a test
//!   fixture, nothing in CI.
//! * **Never fetch one.** No download, no mirror, no "helpful" first-run
//!   prompt.
//! * **Fail soft and silently.** A missing, unreadable, wrong-sized or
//!   wrong-console boot ROM is not an error and not a warning on the hot path;
//!   it is simply the HLE machine, which is what a fresh clone gets.
//!
//! ## Where to put one
//!
//! ```text
//! ${XDG_CONFIG_HOME:-~/.config}/terminalgb/dmg_boot.bin   256 bytes
//! ${XDG_CONFIG_HOME:-~/.config}/terminalgb/cgb_boot.bin   2304 bytes
//! ```
//!
//! `$GB_BOOT_ROM=<path>` overrides both. The pre-rename `gameboy/` directory is
//! still read when it is the one that exists, on the same terms as the config
//! file.
//!
//! ## Layering
//!
//! [`BootRom`] is pure bytes and is what the core understands;
//! [`Cpu::new_with_boot_rom`](crate::cpu::core::Cpu::new_with_boot_rom) takes
//! one. [`find`] and [`load`] are the *host* half — they read `$GB_BOOT_ROM`,
//! `$XDG_CONFIG_HOME` and `$HOME`, and **nothing in the core ever calls them**.
//! An embedder that hands the core bytes it obtained some other way touches no
//! environment variable and no filesystem path, which is what keeps the
//! embedding claim in `AGENTS.md` true.

#[cfg(feature = "embedded")]
use crate::compat::*;

use crate::mode::GbMode;
#[cfg(not(feature = "embedded"))]
use std::path::PathBuf;

/// Which console's boot ROM this is, decided by its length — the only thing
/// about a dump that identifies it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootKind {
    /// 256 bytes, mapped over `$0000-$00FF`.
    Dmg,
    /// 2304 bytes, mapped over `$0000-$00FF` **and** `$0200-$08FF`; the
    /// cartridge header at `$0100-$01FF` shows through the hole between them,
    /// which is how the Color boot ROM reads the title it hashes for its
    /// compatibility palette.
    Cgb,
}

/// A validated boot ROM image.
#[derive(Clone, Debug)]
pub struct BootRom {
    data: Vec<u8>,
    kind: BootKind,
}

/// Why a candidate image was not accepted. Every one of these is reported to
/// the caller and then, on the automatic path, ignored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootRomError {
    /// Not 256 or 2304 bytes, so it is not a boot ROM of a console we model.
    BadLength(usize),
}

impl core::fmt::Display for BootRomError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BootRomError::BadLength(n) => {
                write!(f, "{n} bytes: a boot ROM is 256 (DMG) or 2304 (CGB) bytes")
            }
        }
    }
}

/// The DMG boot ROM's length, and the size of the window it is mapped into.
pub const DMG_LEN: usize = 0x100;
/// The CGB boot ROM's length: `$0000-$08FF` including the 256-byte hole the
/// cartridge header shows through.
pub const CGB_LEN: usize = 0x900;

impl BootRom {
    /// Validate an image. The only check is its length — the contents are the
    /// user's business, and a boot ROM that does not hand over is a hang the
    /// user can see and diagnose, not something to second-guess here.
    pub fn new(data: Vec<u8>) -> Result<BootRom, BootRomError> {
        let kind = match data.len() {
            DMG_LEN => BootKind::Dmg,
            CGB_LEN => BootKind::Cgb,
            n => return Err(BootRomError::BadLength(n)),
        };
        Ok(BootRom { data, kind })
    }

    pub fn kind(&self) -> BootKind {
        self.kind
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Is this the boot ROM of the console that is about to run?
    ///
    /// A DMG image on a Color console (or the reverse) is not "close enough":
    /// it would map over the wrong addresses, hand over the wrong registers and
    /// hang or mis-boot. Rather than run it, the machine falls back to HLE — a
    /// user with only one of the two dumps still gets a working emulator on
    /// every console, with the real boot on the one it belongs to.
    ///
    /// The Super Game Boy has a boot ROM of its own that we do not model, so an
    /// SGB always takes the HLE path.
    pub fn suits(&self, mode: GbMode) -> bool {
        match self.kind {
            BootKind::Dmg => mode == GbMode::Classic,
            BootKind::Cgb => mode == GbMode::Color || mode == GbMode::ColorAsClassic,
        }
    }

    /// The byte this boot ROM answers at `address`, or `None` where the
    /// cartridge shows through.
    pub(crate) fn byte(&self, address: u16) -> Option<u8> {
        let a = address as usize;
        match self.kind {
            BootKind::Dmg if a < DMG_LEN => Some(self.data[a]),
            // The header hole: `$0100-$01FF` is the cartridge's, which is what
            // lets the Color boot ROM read the title it hashes.
            BootKind::Cgb if a < DMG_LEN || (0x200..CGB_LEN).contains(&a) => {
                Some(self.data[a])
            }
            _ => None,
        }
    }
}

/// Where a boot ROM for `mode` would be found, if the user has put one there.
///
/// `$GB_BOOT_ROM` wins outright and is used for either console — someone who
/// names a file explicitly is telling us which one they mean, and being told
/// "that is the wrong console" is more useful than being silently ignored.
///
/// **Host code only.** Nothing in the emulator core calls this.
#[cfg(not(feature = "embedded"))]
pub fn find(mode: GbMode) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("GB_BOOT_ROM") {
        let p = PathBuf::from(explicit);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    let name = if mode.is_dmg() {
        "dmg_boot.bin"
    } else {
        "cgb_boot.bin"
    };
    config_dir().map(|d| d.join(name))
}

/// Load the boot ROM for `mode`, or `None`.
///
/// Every failure — no file, unreadable, wrong length, wrong console — returns
/// `None`. The reason is available through [`explain`] for a frontend that
/// wants to say something once at startup; the emulator itself never needs it.
///
/// **Host code only.** Nothing in the emulator core calls this.
#[cfg(not(feature = "embedded"))]
pub fn load(mode: GbMode) -> Option<BootRom> {
    explain(mode).0
}

/// [`load`], plus a one-line reason when the answer is `None` **and the user
/// evidently meant to supply one** — an explicit `$GB_BOOT_ROM` that does not
/// work, or a file at the config path that does not.
///
/// Nothing to report when there is simply no file: that is the ordinary case,
/// not a problem, and saying so on every launch would be noise.
#[cfg(not(feature = "embedded"))]
pub fn explain(mode: GbMode) -> (Option<BootRom>, Option<String>) {
    let Some(path) = find(mode) else {
        return (None, None);
    };
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            // A missing file at the *default* location is the normal case.
            if e.kind() == std::io::ErrorKind::NotFound
                && std::env::var_os("GB_BOOT_ROM").is_none()
            {
                return (None, None);
            }
            return (None, Some(format!("boot ROM {}: {e}", path.display())));
        }
    };
    match BootRom::new(data) {
        Err(e) => (None, Some(format!("boot ROM {}: {e}", path.display()))),
        Ok(rom) if !rom.suits(mode) => (
            None,
            Some(format!(
                "boot ROM {} is a {:?} image and this is a {mode:?} console — \
                 ignored",
                path.display(),
                rom.kind()
            )),
        ),
        Ok(rom) => (Some(rom), None),
    }
}

/// `${XDG_CONFIG_HOME:-~/.config}/terminalgb`, or the pre-rename `gameboy`
/// directory when that is the one that exists.
///
/// Deliberately a copy of the rule in `screen::config` rather than a call into
/// it: that module is a *frontend* and lives behind the `tui` feature, and the
/// core may not depend on it (`make embed` builds with neither).
#[cfg(not(feature = "embedded"))]
fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(prefer_existing(
        base.join("terminalgb"),
        base.join("gameboy"),
    ))
}

#[cfg(not(feature = "embedded"))]
fn prefer_existing(current: PathBuf, legacy: PathBuf) -> PathBuf {
    if current.exists() || !legacy.exists() {
        current
    } else {
        legacy
    }
}

#[cfg(all(test, not(feature = "embedded")))]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_real_lengths_are_accepted() {
        assert!(BootRom::new(vec![0; DMG_LEN]).is_ok());
        assert!(BootRom::new(vec![0; CGB_LEN]).is_ok());
        for n in [0usize, 1, 255, 257, 2048, 2303, 2305, 32768] {
            assert_eq!(
                BootRom::new(vec![0; n]).unwrap_err(),
                BootRomError::BadLength(n),
                "{n} bytes must be refused"
            );
        }
    }

    /// A boot ROM belongs to one console. Running a DMG image on a Color
    /// machine would map over the wrong addresses and hand over the wrong
    /// registers, so it is declined and the machine boots HLE instead.
    #[test]
    fn a_boot_rom_is_only_used_on_the_console_it_belongs_to() {
        let dmg = BootRom::new(vec![0; DMG_LEN]).unwrap();
        let cgb = BootRom::new(vec![0; CGB_LEN]).unwrap();
        assert!(dmg.suits(GbMode::Classic));
        assert!(!dmg.suits(GbMode::Color));
        assert!(!dmg.suits(GbMode::ColorAsClassic));
        assert!(cgb.suits(GbMode::Color));
        assert!(cgb.suits(GbMode::ColorAsClassic));
        assert!(!cgb.suits(GbMode::Classic));
        // The SGB's own boot ROM is not modelled, so neither image suits one.
        for m in [GbMode::Super, GbMode::Super2] {
            assert!(!dmg.suits(m));
            assert!(!cgb.suits(m));
        }
    }

    /// The Color boot ROM is mapped in two pieces with the cartridge header
    /// showing through the hole — that is how it reads the title it hashes for
    /// the DMG-compatibility palette.
    #[test]
    fn the_cgb_image_leaves_the_cartridge_header_visible() {
        let mut data = vec![0u8; CGB_LEN];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i & 0xFF) as u8;
        }
        let rom = BootRom::new(data).unwrap();
        assert_eq!(rom.byte(0x0000), Some(0x00));
        assert_eq!(rom.byte(0x00FF), Some(0xFF));
        for a in 0x0100..0x0200u16 {
            assert_eq!(rom.byte(a), None, "${a:04X} is the cartridge's");
        }
        assert_eq!(rom.byte(0x0200), Some(0x00));
        assert_eq!(rom.byte(0x08FF), Some(0xFF));
        assert_eq!(rom.byte(0x0900), None);
    }

    #[test]
    fn the_dmg_image_covers_only_the_first_page() {
        let rom = BootRom::new(vec![0xAB; DMG_LEN]).unwrap();
        assert_eq!(rom.byte(0x0000), Some(0xAB));
        assert_eq!(rom.byte(0x00FF), Some(0xAB));
        assert_eq!(rom.byte(0x0100), None);
        assert_eq!(rom.byte(0x0200), None);
    }
}

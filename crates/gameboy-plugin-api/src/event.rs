//! Something the machine did.

/// Something the machine did. The categories and their rough rates are the ones
/// [`docs/debugging.md`](https://github.com/Alchemy86/gameboy/blob/main/docs/debugging.md)
/// enumerates; only the two *cheap* ones are implemented here, because the hot
/// ones (instruction-retired, memory-access) are explicitly specified there as
/// ring-buffered and behind the `debug` cargo feature. Adding them later does
/// not change this enum's existing variants.
///
/// This enum is `#[non_exhaustive]`: a host may deliver a category a plugin
/// built against an older API does not know, and that plugin must ignore it
/// rather than fail to compile. Match with a `_ => {}` arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A frame finished. ~60/s, trivial to emit.
    Frame { number: u64 },
    /// One byte was exchanged on the link cable: the game sent `sent` and
    /// received `received`. A few thousand per second during a Pokémon trade,
    /// a handful per second for a Blargg test ROM.
    SerialTransfer { sent: u8, received: u8 },
}

impl Event {
    /// The event's category name — the string an external subscriber filters on.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Frame { .. } => "frame",
            Event::SerialTransfer { .. } => "serial",
        }
    }

    /// Serialise as one NDJSON record, the same shape the emulator's telemetry
    /// stream uses, so the wire transport described in `docs/debugging.md` is a
    /// change of pipe rather than of contract. Hand-rolled: this crate takes no
    /// serde dependency (see AGENTS.md "no new crate dependencies").
    pub fn write_json(&self, out: &mut String) {
        use std::fmt::Write as _;
        match self {
            Event::Frame { number } => {
                let _ = write!(out, "{{\"ev\":\"frame\",\"number\":{number}}}");
            }
            Event::SerialTransfer { sent, received } => {
                let _ = write!(
                    out,
                    "{{\"ev\":\"serial\",\"sent\":{sent},\"received\":{received}}}"
                );
            }
        }
    }
}

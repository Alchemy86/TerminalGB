# GB-Link (vendored, source)

Real hardware that lets a Game Boy Advance running Pokémon FireRed or LeafGreen join a
Nintendo Switch's own Trade Center or Colosseum over the Switch's local wireless (LDN),
either over three wires or over USB with no computer in between. Vendored here to read
and evaluate against this project's own link-cable work, after the captain shared a video
of it in action and asked the crew to investigate.

## Upstream and canonicity

Two upstream repos, both under the `GB-Link` GitHub org:

| Directory here | Upstream | Ref | Commit | License |
|---|---|---|---|---|
| `switch-ldn/` | <https://github.com/GB-Link/GB-Link-Switch-LDN> | `main` | `f3266330f83fc6a017e18c4a9ba2c4efb593a00a` | AGPL-3.0, LDN-protocol pieces GPL-3.0 |
| `firmware/` | <https://github.com/GB-Link/GBLink-Firmware> | `master` (tag `v2.2.3`) | `4aef46a1aff688cc0a68df666858eccb9b5ae291` | GPL-3.0 |

Exact pins, dates and what was and wasn't vendored are in [`gblink.lock`](gblink.lock).

**Confirmed canonical before vendoring.** A fork, `xxjeysonxx/GB-Link-Switch-LDN`,
exists and was created the same day as this pin (2026-09-24) with commits still landing
the next day; `gh api` shows it 4 commits ahead of and 0 behind the `GB-Link` org repo —
an in-flight contributor branch, not a more-maintained fork. The `GB-Link` org repo was
pushed to as recently as 2026-09-24 itself and is 0 commits behind anything upstream of
it. The project site `switch.gblink.io` (linked from the org repo's own README) points
back at the same org, not at the fork. The org repo is the real, currently-maintained
source.

## What is vendored, and why source rather than an artefact

Everything else in `third_party/` (`gbselftest/`, `dmg-acid2/`, `atlasgb/`) vendors a
**built artefact** — a ROM, because the point of those is to run something built by
someone else and score our own emulator against its independent verdict. That reasoning
does not apply here: there is nothing of GB-Link's to *run* against this emulator, and
the point of vendoring it is to **read the actual mechanism** and report on it honestly,
so this vendors real source — C/C++, not a compiled image. That changes the shape of the
lock file: there is no single build artefact to checksum, so the pin is the upstream
commit SHA (`git rev-parse HEAD` at clone time) for each repo, the same thing a checksum
would prove for an artefact — that a later refresh, deliberate or not, is visible as a
diff against a stated commit.

`switch-ldn/` carries only the ESP32-side protocol source (`firmware/common/*.c/h`) plus
each repo's own README and LICENSE — not the browser client (`web/`), not the desktop
app (`host/`), not the third-party-of-third-party `zstd` library it vendors, and not the
per-chip build glue in `firmware/ESP32*/main`. `firmware/` (from the separate
`GBLink-Firmware` repo) carries the whole Zephyr application source tree under `src/`
(under 30k lines total) and drops the board/toolchain/CI plumbing around it. See
`gblink.lock` for the itemised list.

**Do not edit anything under `switch-ldn/` or `firmware/`.** They are a frozen read-only
copy of someone else's repository at a stated commit, same as every other directory in
`third_party/`.

## The real mechanism

This is two separate pieces of hardware doing two separate jobs, joined by a wire or a
USB cable:

1. **`firmware/`** (a Raspberry Pi Pico / RP2040, or an STM32, the "GB-Link" adapter)
   sits in the GBA's own link port and speaks the GBA's real serial protocol at the
   electrical level. `firmware/src/layers/gbLinkLayer.c` configures a PIO state machine
   to bit-bang exactly the SPI-like signalling a Game Boy/GBA link cable uses — 8-bit
   transfers, CPOL=1/CPHA=1, clock on `SCK`, data on `SIN`/`SOUT`, with a documented
   clock divider (`4058.838f / 128.0f`) chosen to match the real cable's bit rate. The
   application layer above it (`firmware/src/module/gb.cpp`, `firmware/src/module/link.cpp`,
   `firmware/src/sections/tradeConnection.cpp`, `firmware/src/payloads/pokemon.cpp`)
   speaks the actual GBA link protocol used by FireRed/LeafGreen's trade and battle
   negotiation — the `LINKCMD_*`/`LINKTYPE_*` constants in `firmware/src/link_defines.h`
   are the real command codes from that protocol (`LINK_MASTER_HANDSHAKE 0x8FFF`,
   `LINK_SLAVE_HANDSHAKE 0xB9A0`, trade-slot commands, battle-type selectors), and
   `firmware/src/layers/masterClock.hpp` runs the periodic/transmission timers that keep
   the adapter's side of the exchange synchronised to the GBA's own clock. The adapter
   relays what it captures over UART/USB to the ESP32 side (`transport_uart.c` /
   `transport_usb_serial_jtag.c` in `switch-ldn/firmware/common/`) in 64-byte chunks.

2. **`switch-ldn/firmware/common/pia_bridge.c`** is the actual bridge. It frames what
   comes from the GBA as **RFU1** packets — the real framing of the GBA Wireless Adapter
   protocol (`RFU1_HOST_SEND`, `RFU1_CLIENT_SEND`, `RFU1_CONNECT_REQ`/`_ACK`, the state
   machine `BR_IDLE → BR_SCAN → BR_AUTH → BR_MEMBER → BR_NET → BR_RUN`) — and on the other
   side speaks the Switch's own local-wireless protocol (LDN) through `ldn_control.c`,
   `ldn_wire.c`, `ldn_udp.c`, `ldn_keys.c` and `ldn_probe.c`, using the four key values the
   web client reads out of the user's own `prod.keys` to decrypt LDN traffic. The comment
   at the top of `pia_bridge.c` even documents which activity group ids (trade / single
   battle / double battle) come straight from `pokefirered`'s `union_room.h` constants. In
   short: the ESP32 makes itself look like an ordinary Switch peer to the real Switch's
   LDN network, and makes itself look like a GBA Wireless Adapter to the real GBA — a
   protocol *translator* sitting between two consoles that have never spoken to each
   other's transport before, each side convinced it's talking to a normal peer of its own
   kind. Nothing here emulates a Game Boy or a Switch; both consoles are real and running
   their own real firmware.

## Licence and what that means for this project

TerminalGB is MPL-2.0 (see the repo root [`LICENSE`](../../LICENSE)) — a permissive,
file-level copyleft licence. `GBLink-Firmware` is GPL-3.0. `GB-Link-Switch-LDN` is
AGPL-3.0, with its LDN-protocol-specific files under plain GPL-3.0
(`switch-ldn/licenses/LDN-GPL-3.0.txt`).

Vendoring the source read-only here, as reference material that is never built and never
linked into TerminalGB's own binary, creates no licence obligation at all — nothing of
ours combines with it. That is the entire point of doing it this way rather than, say,
copying a function into a plugin. **If any of this code were ever actually copied or
adapted into TerminalGB's own source** (`src/`, `plugins/`, `crates/`), the file or module
it landed in would need to become GPL-3.0- or AGPL-3.0-compatible — MPL-2.0 does permit
combining MPL code into a GPL/AGPL-licensed larger work (its "secondary licence" clause,
§3.3), but not the reverse: TerminalGB as a whole could not absorb GPL/AGPL code and stay
MPL-2.0 as a project. Concretely: reading this for ideas and writing an independent
implementation is fine and is what this directory is for; copying code out of it into
anything TerminalGB ships is not, without separately relicensing the receiving file.

## Is any of it reusable here?

Honestly: **conceptually adjacent, nothing directly portable**, and the direction of the
problem is inverted from what this project does.

GB-Link makes new hardware pretend to be an old console (a GBA with a Wireless Adapter)
so a *real* modern console (the Switch) accepts it as a peer over a protocol
(LDN/RFU1/GBA-wireless) that has no counterpart anywhere in this emulator — there is no
Switch emulation here, planned or otherwise, so `switch-ldn/`'s entire LDN-facing half
(`ldn_control.c`, `ldn_wire.c`, `ldn_udp.c`, `ldn_keys.c`, `ldn_probe.c`) has no
application to this project at all. This project's own [`plugins/gen1-link/`](../../plugins/gen1-link/)
does the opposite: it makes *software inside an emulator* pretend to be a second Game Boy
plugged into a link cable, so a real Game Boy game's own link-cable code is fooled from
the inside. Different consoles (Game Boy vs. GBA), different transport (a DMG/GBC serial
port vs. a GBA Wireless Adapter's RFU protocol), and — the part that actually rules out
reuse — different sides of the wire: GB-Link's adapter must physically drive or follow a
real electrical clock in real time (`firmware/src/layers/gbLinkLayer.c`'s PIO program,
`masterClock.hpp`'s hardware timers), while `plugins/gen1-link/src/protocol.rs` solves the
same-shaped problem — "who drives the clock" — entirely in software, by choosing to always
answer the emulated game's own `ESTABLISH_CONNECTION_WITH_INTERNAL_CLOCK` offer with "you
take the clock" (see the comment header of `protocol.rs`, `$02` in `SB`), which needs no
timing tolerance or clock recovery because there is no real electrical clock to
synchronise to at all. That is a real, specific parallel — both projects independently hit
"who owns the clock" as the crux of bridging a link cable — but the two solutions don't
share a line of code and couldn't: one is PIO assembly bit-banging a real signal, the
other is picking which byte to hand back to an emulated CPU. If this project ever adds a
Gen 2 link plugin, the same holds: Gen 2's DMG/GBC serial protocol is the same electrical
shape GB-Link's `gbLinkLayer.c` also bit-bangs (both are the same 8-bit clocked-SPI GBC
link port, unlike GBA's separate multiplayer/wireless bus), so it is worth rereading for
timing values (the PIO clock divider, the external-clock-detection trick on the `SD` pin)
if this project ever emulates *real hardware timing behaviour* on the link port rather
than the current software shortcut — but that is a future "worth rereading," not code to
pull in today.

# Interoperability: TerminalGB against somebody else's BGB link implementation

`src/bgblink.rs` implements a protocol whose entire value is that **other
people already speak it**. A test where both ends are ours proves the driver
and proves nothing about that. This directory is the other half: it drives the
BGB link server from
[`Lorenzooone/PokemonGB_Online_Trades`](https://github.com/Lorenzooone/PokemonGB_Online_Trades)
(MIT) **unchanged, as a library**, and checks that bytes cross.

That tool is the one worth interoperating with. Its
`utilities/bgb_link_cable_server.py` and its `usb_trading.py` put the *same*
trading logic behind two different byte channels — an emulator over this
protocol, or a real Game Boy on a USB link adapter — so an emulator that talks
to the first can trade with a cartridge on the second, over the internet.

## Running it

```sh
make bgb-interop
```

(which is `fetch.sh`, then `cargo build … --example bgb_link`, then
`python3 tools/bgb-interop/interop.py`)

It is **not** in CI, for the same reason SameSuite is not: it fetches somebody
else's repository at run time and needs a Python interpreter. Run it by hand
when `src/bgblink.rs` changes, and record what it printed.

## What it checks

The Python side is the **master** (it sends sync1 / packet 104 and expects
sync2 / 105 back), which is the role that reaches real hardware. The emulator
runs `bgb_link --echo`, a fifteen-instruction cartridge that answers the cable
and offers the **complement** of whatever arrived. So the two verdicts are
independent:

* the emulator's `$C000` onwards must hold exactly the bytes Python sent, and
* Python must receive `~b` for each byte `b` it sent one step earlier.

An echo of the *same* byte would not distinguish a working link from a peer
reading its own transmission back. The complement is a value only the emulated
console can have produced.

## One property of the counterparty worth knowing

`BGBLinkCableServer` sends no data at all until its `can_go` flag is set, and
that only happens when it receives a **sync3 with `b2 = 1`** — the client saying
"I am not doing a passive transfer". A cartridge satisfies that without trying,
because it boots before it touches the link port; a test ROM that arms on its
first instruction never would. Hence the deliberate startup delay in
`echo_rom()` (`examples/bgb_link.rs`), and hence the leading `0x00` in what
Python receives: the server's own `_handle_sync3` records that acknowledgement
as a received byte.

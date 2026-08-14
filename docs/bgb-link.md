# The link cable over a network — BGB's protocol

TerminalGB can put its link port on a TCP socket. The far end can be:

* **another copy of TerminalGB**, on this machine or across the internet
  (**measured** — see §4);
* **`PokemonGB_Online_Trades`' link server** (**measured** — see §4);
* **BGB**, or any other emulator implementing the same protocol. *Inferred from
  the published specification, not measured*: BGB is a Windows program and
  nobody here has run one against this;
* **a real Game Boy.** *Inferred, not measured — there is no link adapter on
  this machine.* Not directly — through
  [`Lorenzooone/PokemonGB_Online_Trades`](https://github.com/Lorenzooone/PokemonGB_Online_Trades)
  (MIT), whose `emulator_trading.py` speaks this protocol on one side and whose
  `usb_trading.py` drives a USB link adapter with a cartridge in it on the
  other, with a WebSocket relay between the two players.

That last one is the reason this exists, and it is why it was worth building
before any hardware transport: **it costs no hardware purchase at all.** The
person with the cartridge buys an adapter; the person with TerminalGB buys
nothing. The path is established by construction rather than demonstrated —
`emulator_trading.py` and `usb_trading.py` bind the *same* `RBYTrading` /
`GSCTrading` logic to different `sendByte`/`receiveByte` pairs, and the first of
those two is what §4 drives — but the last hop has not been run here and this
page does not pretend otherwise.

Implementation: [`src/bgblink.rs`](../src/bgblink.rs) (the protocol) over
[`src/link/transport.rs`](../src/link/transport.rs) (the driver). Behind the
`bgblink` cargo feature, which is in `default` and out of
`--no-default-features`; it adds **no third-party crate** — the whole thing is
`std::net` and eight bytes at a time.

---

## 1. Using it

`config.toml`:

```toml
# one side listens...
link_net = listen:0.0.0.0:8765
# ...and the other connects. Without a prefix, `connect:` is assumed.
link_net = connect:192.0.2.10:8765
```

`GB_LINK_NET=…` overrides the config key for one session. The Status panel
grows a **`Net`** row: `waiting` until somebody connects, then a transfer
count, then `FAULT` if the link breaks. A climbing `missed` count beside it (
`12 tx 4!`) is the diagnostic that matters — see §5.

**A Game Boy has one link port**, and three things now want it: an
auto-discovered peer on this machine (`link = auto`, the default), a second
console inside this process (`link = <rom>`), and this. `link_net` **wins** —
it is the only one of the three a user types on purpose — and whichever it
displaces is reported in the warnings rather than silently dropped.

### Trading with a real Game Boy

You need nothing but the software. The other player needs a link adapter
(the [Pico board](https://github.com/agtbaskara/game-boy-pico-link-board) is
MIT and about US$15 in parts; GB-Link sells a finished one).

```sh
pip install websockets
git clone https://github.com/Lorenzooone/PokemonGB_Online_Trades
cd PokemonGB_Online_Trades && python3 ./emulator_trading.py   # pick 2-player
```

Its menu asks which host and port to listen on; put the same address in
`link_net = connect:…` and start TerminalGB. The other player runs
`usb_trading.py` against their adapter. Both tools build the *same* trading
logic over different byte channels, which is the whole trick.

### Two copies of TerminalGB

```sh
cargo run --release --features image,plugin-gen1-link,plugin-trainer \
    --example bgb_link -- --rom blue.gb --save fixture.sav \
    --name a --listen 127.0.0.1:8765 --party red  --trade --expect-ot BLUE

cargo run --release --features image,plugin-gen1-link,plugin-trainer \
    --example bgb_link -- --rom blue.gb --save fixture.sav \
    --name b --connect 127.0.0.1:8765 --party blue --trade --expect-ot RED
```

Both processes drive Pokémon Blue into the Cable Club and trade. See §4 for
what that measured.

---

## 2. The protocol, in full

Eight bytes per packet: four bytes then a little-endian `u32`. Reference:
<https://bgb.bircd.org/bgblink.html>.

| `b1` | name | `b2` | `b3` | `b4` | `i1` |
|---|---|---|---|---|---|
| 1 | version | major (1) | minor (4) | patch (0) | 0 |
| 101 | joypad | button + state | 0 | 0 | 0 |
| 104 | sync1 — **master data** | data byte | `SC` value | 0 | timestamp |
| 105 | sync2 — **slave data** | data byte | `$80` | 0 | 0 |
| 106 | sync3 — ack / timestamp | 1 = ack, 0 = timestamp | — | — | timestamp |
| 108 | status | bit 0 running, 1 paused, 2 reconnect | 0 | 0 | 0 |
| 109 | wantdisconnect | 0 | 0 | 0 | 0 |

One side listens, the other connects. Both send a version packet and verify
what comes back; a status packet is sent **in response to version**, and
`TCP_NODELAY` is set because a link byte is eight bytes long and Nagle would
sit on it waiting for company that never comes.

Three of those packets carry everything that matters:

* a console driving the clock sends **sync1**;
* a console armed for an *external* clock answers **sync2** with the byte in
  its own shift register — one complete exchange, after which each side holds
  what the other started with, exactly as eight clock edges on a real cable
  leave them;
* a console that was **not** armed answers **sync3 with `b2 = 1`**, and the
  master shifts in the idle-high `$FF`.

That maps one-to-one onto `Serial::link_answer_master` and
`Serial::link_clock_slave`, which [`docs/link-cable.md`](link-cable.md)'s
two-console work already built. This transport writes no new serial path.

### Packet boundaries are not packet boundaries

"Multiple commands may be in a single TCP packet, and a command may be split
over multiple TCP packets." Hence an accumulating buffer, not an eight-byte
read. `a_packet_split_across_reads_is_reassembled` sends two packets one byte
at a time.

---

## 3. The two design decisions worth arguing about

### 3a. The console is **stalled**, and that is the honest reading of timestamps

A master's transfer completes on its **own** shift clock in about a
millisecond of emulated time whether or not anything answers (Pan Docs,
*Serial Data Transfer*). A far end across a socket cannot reliably answer
inside that window. A driver that let the console run would hand it `$FF`
every single time, and no link would ever carry a byte.

BGB's answer is timestamps: each side keeps the difference between its clock
and the peer's, and uses it "so each side can, at the right times, wait for the
remote side". Our answer is to do the waiting directly —
`TransportLink::run_frame` stops paying cycles to the CPU until the answer
arrives. Emulated time really has not passed, which is why the timestamp we
report is derived from **emulated** T-cycles (2 T = 1 tick of the protocol's
2 MiHz clock) and not from the wall clock.

We send our timestamp and record the peer's, and gate nothing on the
difference. That is not a corner cut in a hurry: `PokemonGB_Online_Trades`
"cheats" the same way — its own comment reads *"Cheat, and say we are exactly
in sync with the client"* — and it completes real trades with real cartridges
at **10 ms per byte from a Python thread that does not even set
`TCP_NODELAY`**. These games are turn-based. Neither side proceeds until it has
the other's byte, so the protocol's request/response structure *is* the
synchronisation.

Stalls are counted (`TransportStats::stalls`) and visible. In the measured
trade below the master stalled once per byte and the slave never stalled at
all, which is exactly what the design predicts and a useful thing to be able to
see.

### 3b. An **incoming** byte is held, not refused

When the far end clocks us, our console may not have armed `SC = $80` yet — it
is a whole console running its own program and the far end has no idea which
instruction it is on. Refusing immediately would drop the byte, and a dropped
byte mid-trade is the dangerous state.

So an incoming byte is held for up to `SLAVE_ARM_WINDOW` (two frames) while the
console keeps running, and only refused when it plainly is not going to take
it. Refusals are counted as `missed` — see §5.

### 3c. Nothing blocks. Ever.

The socket is non-blocking from the moment it exists, `accept` included, so a
console configured to *listen* runs normally while it waits for a peer. This is
the rule the audio player cost 17.9 ms of a 16.74 ms frame to learn
([`docs/audio.md`](audio.md) §4c) and the video sink cost a hung startup
(`AGENTS.md`, "no media sink may ever block the emulator"). The one bounded
blocking call is `connect_timeout` at open, before the terminal exists.

---

## 4. What was measured

### A Gen 1 trade, over TCP, between two processes

Two `examples/bgb_link` processes, real sockets, Pokémon Blue on both, driven
into the Cable Club by the shared trade bot:

```
[a] link: 1786 transfers, 0 unanswered, 0 missed, 0 timeouts, 1785 stalls, over 22.0s
[b] link ended: Setup("the peer asked to disconnect")
[b] link: 1786 transfers, 0 unanswered, 0 missed, 0 timeouts, 0 stalls, over 21.0s

[a] party: RATTY  … OT RED     ROCKY  … OT BLUE
[b] party: FLAP   … OT BLUE    GLIMMER … OT RED  <- would be SHINY if traded to Gen 2
```

**Each console ends up holding a Pokémon whose original trainer is the other
console's player** — a value neither process wrote and neither could have
invented. `--expect-ot` makes that the exit status, so a shell script is the
judge. Around 1,550-1,800 exchanges depending on how the two bots interleave,
with nothing unanswered, nothing missed and nothing timed out; the master
stalls once per byte and the slave never stalls, which is what §3a predicts.
Neither side faults, because the one that finishes first says goodbye.

### The bug this run found, and a unit test could not

`BgbLink::disconnect` was written as an **inherent** method and never added to
the `impl LinkTransport for BgbLink` block. The trait's default body is a no-op,
so a frontend's clean shutdown — which goes through `dyn LinkTransport` —
silently sent nothing, and every peer recorded a **broken** link and stopped
writing its save. The unit test passed throughout, because it called the
inherent method on the concrete type.

Two things came out of it. The test now goes through
`Box<dyn LinkTransport>` (teeth proven: removing the override turns it red),
and `examples/bgb_link` grew `--settle`, because the two consoles do not finish
together and whichever finishes first has to keep the cable plugged in — a run
that had traded correctly once reported a party entry read mid-write
(`LIMMER`, OT `ED`, level 0) for want of it.

### Against somebody else's implementation

Both ends being ours proves the driver and proves nothing about interoperating.
`tools/bgb-interop/` drives
`PokemonGB_Online_Trades`' `utilities/bgb_link_cable_server.py` **unchanged, as
a library**, with the Python side as master:

```
took 3 probe(s) before the console had armed its port
OK: the master received ['0xff', '0xfe', '0xfd', '0x2', '0x1', '0xff', '0x0',
                         '0xaa', '0x5a', '0xc3', '0x3c', '0xef']
    — the complement of each byte it sent, one step behind
OK: the emulator received ['0x1', '0x2', '0xfd', '0xfe', '0x0', '0xff', '0x55',
                           '0xa5', '0x3c', '0xc3', '0x10', '0x80']
    — exactly what the master sent, in order
interop: PASS
```

Two independent verdicts, because the emulator runs an echo cartridge that
offers the **complement** of whatever arrived: a plain echo could not tell a
working link from a peer reading its own transmission back. **Teeth proven**:
making `answer_incoming` send `$00` instead of the console's byte turns it red
(exit 1) and restoring it turns it green again.

Not in CI, for the same reason SameSuite is not — it fetches somebody else's
repository at run time and needs Python. Run it by hand when `src/bgblink.rs`
changes:

```sh
make bgb-interop
```

### What has **not** been tested

* **BGB itself.** It is a Windows program; nobody has run it against this. The
  protocol is implemented from its published specification and verified against
  a second independent implementation of that specification, which is the
  strongest claim available here — but it is not the same claim.
* **A real Game Boy.** No adapter exists on this machine. The path is
  established (the Python tool's two front ends are interchangeable by
  construction) but not demonstrated.

---

## 5. Debugging a link that is not moving

Read the `Net` row, and read `missed`.

| symptom | meaning |
|---|---|
| `waiting` for ever | nobody connected. Check the address, and that exactly one side is `listen:`. |
| transfers climb, `missed` 0 | working. |
| `missed` climbs, transfers flat | **both ends have settled into the same role.** Nothing will ever move: two consoles both driving the clock shift in `$FF`, two both listening deadlock. Every link protocol's handshake exists to break this; if yours cannot, one game is not in its link menu. |
| `unanswered` climbs | the far end is connected but never armed. Normal while a game is not in its link menu — Gen 1's handshake spends its whole idle time here. |
| `timeouts` climb | the far end connected and then stopped answering. |
| `FAULT` | the link broke while live. **The battery save is no longer being written**, deliberately — see §6. |

`stalls` is not a fault indicator: expect roughly one per transferred byte on
whichever side is driving the clock.

---

## 6. Failing safe

A link that breaks *while live* faults the console
(`Gameboy::report_fault`), which poisons the battery so no further save is
written this session. The reasoning is
[`docs/link-cable.md`](link-cable.md)'s: a Gen 1 trade removes a Pokémon from
one party before adding it to the other, so a link that dies between those two
writes leaves whichever side is still running holding a save the player would
rather not keep. We cannot reach the far end's save; we can refuse to write our
own.

Two distinctions keep that from being a nuisance:

* **A link that never came up is not a broken one.** Nothing listening, a
  refused connection, a version mismatch — those cost you the cable and nothing
  else, and the console is untouched.
* **A peer that says goodbye first is unplugged, not broken.** `wantdisconnect`
  (packet 109) is the only thing on the wire that can tell "the player quit"
  from "the emulator died". TerminalGB sends one when a session ends — from the
  frontend's shutdown path, **never from `Drop`**, for the reason
  `src/mbc/battery.rs` records at length — and honours one it receives, unless
  a byte was in the air at the time.

---

## 6a. Dead ends, so nobody repeats them

Four things were tried and were wrong. Each cost real time and each is now a
test.

1. **Sending the goodbye and dropping the socket.** Closing a socket that still
   has unread data in its receive queue sends an **RST**, and an RST discards
   everything already in flight — including the `wantdisconnect` written a
   microsecond earlier. *Measured*: the peer read `Connection reset by peer`,
   faulted, and stopped writing its save. `BgbLink::graceful_close` now writes,
   drains, half-closes, and waits up to `LINGER` (250 ms) for the peer's FIN.
2. **Recording the hang-up as it is seen.** A polite peer writes its goodbye and
   closes, so both land in **one `read(2)`**. `close_as` deliberately keeps the
   *first* reason it is given, so noting the EOF before parsing the buffer
   overwrote the goodbye and turned every clean shutdown into a broken link.
   Parse first, then close. `went_live` is latched where the phase becomes
   `Live` for the same class of reason — asking "was this link ever up?" must
   not depend on where in a poll you ask.
3. **Writing `disconnect` as an inherent method only.** See §4. The trait's
   default body is empty, the driver calls it through `dyn LinkTransport`, and
   the result was a silent no-op that a unit test on the concrete type could not
   see.
4. **Starting the interoperability payload as soon as the counterparty is
   ready.** The emulated console is still booting and has not armed its port, so
   the first few bytes are (correctly) refused. The first attempt lost `0x01`
   and `0x02` off a twelve-byte payload and reported `3 missed`. The fix is
   **not** a `sleep`: the harness probes with `0x00` until the console's
   pre-arm sentinel `$A5` comes back, which is deterministic. That in turn is
   why `0x5A` is absent from the payload — the echo ROM answers with the
   complement, so it can only offer `$A5` again after receiving `$5A`.

## 7. What is deliberately not implemented

* **Joypad packets (101).** They exist so one BGB can drive another's buttons.
  Accepting them would let a network peer play the game on this console, which
  is not a link cable and is not something a trade partner should be able to
  do. They are parsed and dropped.
* **Reconnection.** Our status packet advertises no reconnect support, so a
  well-behaved peer will not send a `wantdisconnect` expecting to come back.
* **Timestamp-gated waiting.** See §3a — the waiting is real, it is just done
  by stopping the CPU rather than by comparing clocks.

---

## 8. Licensing

BGB's protocol is publicly documented and interoperating with a documented
protocol is fine; `src/bgblink.rs` is written from that page and reproduces no
code. `PokemonGB_Online_Trades` is **MIT**, is used unchanged as a library by
the interoperability harness, and is fetched at run time rather than vendored —
the position `tools/fetch-test-suites.sh` takes for the test ROMs, and the one
`CONTRIBUTING.md` requires. `gb-link-firmware-reconfigurable` and
`GBLink-Firmware` are **GPL-3.0** and nothing here draws on them at all.

---

## See also

* [`docs/link-cable.md`](link-cable.md) — two whole consoles on one wire, in one
  process. The same `Serial` half; no network.
* [`docs/gen1/link-from-outside.md`](gen1/link-from-outside.md) — driving the
  Gen 1 link protocol from Rust, which is the answer when you own one cartridge.
* [`plugins/gen1-link`](../plugins/gen1-link) — impersonating a second console.

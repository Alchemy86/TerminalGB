# Bridge firmware

Firmware for an ESP32 that sits next to a GB-Link running mode `0x07`
(GBA Wireless Adapter). It joins a Nintendo Switch's local-wireless (LDN) room, speaks
the Switch's session protocol, and relays the GBA's wireless-adapter traffic over a
2-wire UART, so a real GBA running FireRed or LeafGreen can trade and battle with the
Nintendo Switch release of the same game. No PC is involved once it is flashed. The
[web client](../web/README.md) installs prebuilt images of this firmware, stores the
keys, and can stand in for the UART when both boards are on USB.

Trades and single battles are verified on hardware. A double-battle group is
advertised as well and has not been tested.

## Hardware

One source tree (`common/`) builds for four chips; each chip's folder holds only its
target, console transport, pinned Wi-Fi driver hash and defaults.

The original ESP32 and the ESP32-S3 are the recommended chips.

| Folder | Console | To the GB-Link's GP9 | To its GP8 |
| --- | --- | --- | --- |
| `ESP32` | UART0 through the board's USB bridge, 921600 baud | GPIO17 (TX2) | GPIO16 (RX2) |
| `ESP32-S3` | native USB Serial/JTAG | GPIO2 | GPIO1 |
| `ESP32-C6` | native USB Serial/JTAG | GPIO2 | GPIO1 |
| `ESP32-C3` | native USB Serial/JTAG | GPIO5 | GPIO4 |

The original ESP32 has the least memory of the four, about 19 KB free once running.

The GP9 column is the board's transmit pin (`CONFIG_PICO_LINK_TX_GPIO` in the folder's
`sdkconfig.defaults`) and the GP8 column its receive pin (`CONFIG_PICO_LINK_RX_GPIO`);
ground goes to ground, and the link runs at 921600 baud. The direction matters: nothing
detects a crossed pair, and the link simply stays silent. `LDN_PICO_PROBE` tells which
of the two pins has a transmitter on the far end (wired correctly, that is the RX pin),
and `LDN_PICO_SWAP` exchanges the two pins until the next restart, which tells a crossed
pair from a dead link without rewiring.

Boards used: Seeed Studio XIAO ESP32-S3 (D0 = GPIO1, D1 = GPIO2), M5Stack AtomS3 (G1,
G2) and a DOIT ESP32 DEVKIT V1, which like most original-ESP32 boards labels GPIO17 as
TX2 and GPIO16 as RX2. The XIAO ESP32-S3 has no onboard antenna: clip the supplied
antenna onto its U.FL connector. The XIAO ESP32-C6 routes its antenna through an RF
switch, which `ESP32-C6/main/xiao_c6_board.c` enables; `LDN_ANTENNA 0|1` selects onboard
or external.

## Build and flash

The LDN join drives a private Wi-Fi driver interface whose ABI is not stable, so the
build is pinned: ESP-IDF v6.1 at commit `fff9895c82d744c7237be8847347bdd1b07c6643`,
and each chip's `main/CMakeLists.txt` checks the SHA-256 of that release's
`libnet80211.a` for its target.

```
tools/build.sh ESP32-S3
tools/build.sh ESP32-S3 flash /dev/ttyACM0
```

The first argument is the chip folder. With a plain ESP-IDF shell, `idf.py -p <port>
flash` from a chip folder works too. For other flashing tools write:

| Address | File |
| --- | --- |
| `0x0` | `bootloader/bootloader.bin` |
| `0x8000` | `partition_table/partition-table.bin` |
| `0x10000` | `ldn_bridge_<chip>.bin` |

### Keys

The four LDN keys live in the board's NVS, not in the firmware, so a freshly flashed
board needs them once over its console. The web client does this from a `prod.keys`
dropped on the page; by hand, the values come from a Switch's `prod.keys`:

```
LDN_KEY aes_kek_generation_source <32 hex>
LDN_KEY aes_key_generation_source <32 hex>
LDN_KEY master_key_00 <32 hex>
LDN_KEY master_key_12 <32 hex>
LDN_KEYS
```

`LDN_KEYS` reports which slots are filled and never echoes a key. Erasing flash wipes
them; reflashing the three images does not. A board without keys stops its bridge the
first time it finds a room, and `LDN_BRIDGE_START` starts it again once they are stored.

## The GB-Link side

Mode `0x07` (GBA Wireless Adapter) is part of the
[GB-Link firmware](https://github.com/GB-Link/GBLink-Firmware). The bridge
UART is always live: status and data go back to whichever side sent the last command,
so USB stays available to a host PC. The mode is never entered on its own; this
firmware asks for it shortly after boot and keeps asking until the adapter answers. In
the mode the adapter reports twice a second, so three seconds without a frame mean it is
no longer in the mode for this board (it was powered or plugged in later, restarted, or
has been answering a USB host since), and the asking starts again. Neither board has to
be powered first. `tools/pico_link_test.c` runs this on a host against a scripted
adapter; the build command is in its header.

The adapter's transfer path is relocated into RAM (`zephyr_code_relocate` in that
firmware's top-level `CMakeLists.txt`). After each 32-bit word the GBA can clock the next one
about 40 µs later, and the reply has to be staged inside that gap. Run in place from
flash, that path shares the RP2040's 16 KB XIP cache with the kernel, the bridge UART
and USB, and a cold pass overruns the gap: the GBA reads a stale word and its link
library restarts the command 130 ms later, which shows up as a link that joins a room
and then stalls. Hardware command `0x4c` reports the worst interrupt pass, the passes
over budget, the tightest gap the GBA left, and the pin the last soft-reset pulse
arrived on.

## How a session runs

1. The bridge scans for a FireRed/LeafGreen room and joins it as a second console.
2. It advertises one group per activity to the GBA — trade, single battle, double
   battle — each under its own device id. A joining game lists only the leader whose
   activity matches what its player chose, and the GBA's connect request names the
   group it picked, so the bridge follows the GBA and does not need to know what the
   Switch is hosting. Host the same activity on the Switch.
3. Traffic is relayed in both directions through `trade_shim`, which repairs the
   differences between a cartridge and the Switch build of the game (below).
4. When the Switch's game leaves the room, the adapter is sent back through mode entry
   so the GBA stops listing the old group, and the board restarts and scans again.
   Closing a room takes the Switch's whole network down; a room hosted afterwards is a
   new network. A join whose handshake does not complete within 10 seconds restarts
   the board as well.

### Link-layer repairs

The room is lockstep with the Switch as the authority, across a link with far more
latency than the games expect. `trade_shim` keeps the two sides consistent:

- **Standby barriers.** The Switch build runs one standby round more than a cartridge
  after a trade; the shim supplies it for the GBA and maps round numbers afterwards.
- **Block transfers.** The parent echoes each child block fragment once. A lost echo
  makes the child resend forever, so missing echoes are supplied, the child's mod-8
  sequence tags are re-stamped at send time, and an unanswered block request is
  repeated.
- **Held-key reports.** The GBA moves its own avatar only when the Switch echoes its
  input back, so every queued report is lag on the GBA's screen. Superseded reports
  (no key, or a d-pad direction) are dropped oldest-first once more than
  `PIA_OUT_SHED_AT` are waiting. Button presses and all trade and battle traffic are
  never dropped.

`tools/trade_shim_test.c` simulates the standby exchange on a host; the build command
is in its header.

## Console

The USB console takes line commands:

- `LDN_BRIDGE_STATUS` — link and session counters, then the shim's own.
- `LDN_SHIM_LOG` — event ring that survives a restart: room entry, standby rounds,
  trades, the adapter's telemetry, and its trace when a GBA resets mid-link.
- `LDN_INFO` — firmware name, version, chip and console transport; no side effects.
- `LDN_STATUS` — link state and counters, with the free heap and the lowest it has been.
- `LDN_RF` — signal strength of the room's advertisements since the last call.
- `LDN_PICO_STATUS` / `LDN_PICO_STATS` / `LDN_PICO_PROBE` / `LDN_PICO_SWAP` — the UART
  link to the GB-Link: status codes and counters, which pin the far end drives, and the
  two pins exchanged until the next restart.
- `LDN_PICO_BOOTSEL` — reboot the GB-Link into its USB bootloader, so a board wired in
  place can be reflashed without its BOOTSEL button.
- `LDN_BRIDGE_STOP` / `LDN_BRIDGE_START` — stop and restart the bridge by hand.
- `LDN_ADAPTER host|uart` — where the adapter is: on the UART, or behind the host, which
  then carries GB-Link frames as message kinds 6 and 7 (binary mode only). Not kept
  across a restart. `tools/host_bridge.py` and the web client use it.

Opening the console resets the chip if the serial tool toggles DTR. Open it with DTR
high and RTS low, then drop DTR. The original ESP32's console runs at 921600 baud
(`tools/build.sh ESP32 monitor <port>` picks that up); what its ROM prints before the
firmware starts is still at 115200.

## Troubleshooting

- **The GBA freezes when the game starts the adapter** (talking to the Direct Corner
  receptionist, or choosing to join or lead a group). The game cannot get a command
  through, resets the adapter and tries again about four times a second, and blocks
  while it does. The cable must be a Game Boy Color one: a Game Boy Advance cable does
  not connect both data lines at once, and this mode is full duplex. It is also the
  only mode where the GBA clocks the link at 2 MHz, and some cables are too slow or
  too noisy for how earlier builds of the GB-Link mode read them. Update the adapter's
  firmware; if it still happens, try another cable. The web client says so while it
  carries the link.
- **The GBA lists the Switch but says the trainer is busy, or sits on "waiting for
  response".** The Switch is holding a dead session from an earlier run. Close the room
  on the Switch and host it again.
- **The GBA never lists the Switch.** Check that the bridge has keys (`LDN_KEYS`), that
  the antenna is attached, and that both consoles picked the same activity.
- **Choppy walking.** Reception: the bridge sheds more reports when the Switch's frames
  arrive late or merged. Move the antenna closer to the Switch.

## Licence

AGPL-3.0, see `LICENSE` at the repository root; the LDN protocol components are
GPL-3.0 (`licenses/LDN-GPL-3.0.txt`).

# Serial — the link cable

> Source: [`src/mmu/serial.rs`](../../src/mmu/serial.rs) · Related: [MMU](mmu.md) · [testing](../testing.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`9162be3`](https://github.com/Alchemy86/gameboy/tree/9162be32ec660ef0c10a9e4164b9b5f6792950a8) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The serial port is the Game Boy's **link cable**: two consoles connected wire-to-wire to
trade one byte at a time (Tetris versus, Pokémon trades, four-player games via an adapter).
One console supplies the clock (the "master"), shifts its byte out one bit per clock, and
shifts the other console's byte in on the same wire.

It earns its own page for a reason far beyond gameplay: **the standard Blargg test ROMs
report their pass/fail result by "printing" text out the serial port.** A headless test
harness reads that serial output to know whether a test passed. So serial is the plumbing
our future test suite depends on — see [testing](../testing.md).

## 2. The real hardware

Two registers (Pan Docs — [Serial](https://gbdev.io/pandocs/Serial_Data_Transfer_(Link_Cable).html)):

- **`SB` (`0xFF01`)** — the 8-bit shift register: the byte being sent, and (as it shifts)
  the byte being received.
- **`SC` (`0xFF02`)** — control: **bit 7** starts a transfer, **bit 0** selects the clock
  source (`1` = internal/this console is master, `0` = external), CGB **bit 1** selects
  fast clock.

A transfer shifts 8 bits; when the internal clock is used the rate is **8192 Hz**
(≈1 KB/s), or ~256 KB/s in CGB fast mode. When all 8 bits are done, `SC` bit 7 clears and
the **Serial interrupt** (`IF` bit 3) fires. With no cable connected, an internal-clock
transfer shifts in `0xFF` (all ones).

## 3. History and versions

| Model | Serial differences |
|---|---|
| **DMG / MGB** | 8192 Hz internal clock only. |
| **SGB** | Link functionality present; the SNES doesn't expose a physical link. |
| **CGB** | Adds the **fast clock** (`SC` bit 1) at ~256 KB/s for CGB-aware games; still backward-compatible with the DMG rate. |

The link *protocol* is unchanged across models; a DMG and a CGB can link for DMG games.

## 4. How it works here

**The `Serial` struct** — [`src/mmu/serial.rs:7`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L7): `data` (SB),
`control` (SC), a `callback`, and `interrupt`.

**Registers** — `wb`/`rb` ([`serial.rs:24`,`40`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L24)). Writing `SC`
with **bit 7 and bit 0 both set** (`v & 0x81 == 0x81`, an internal-clock transfer) hands
the outgoing byte to the callback and, if it returns a byte, stores it into `SB` and raises
the serial interrupt:

```rust
0xFF02 => {                                   // serial.rs:27
    self.control = v;
    if v & 0x81 == 0x81 {
        if let Some(v) = (self.callback)(self.data) {
            self.data = v;
            self.interrupt = 0x8              // IF bit 3
        }
    }
}
```

`SC` reads back with the unused bits set (`control | 0b01111110`,
[`serial.rs:43`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L43)).

**The capture hook exists but is unused.** There is a `SerialCallback` type, a
`new_with_callback`, and `set_callback`/`unset_callback`
([`serial.rs:1`,`15`,`66`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L1)) — exactly the seam a Blargg harness
needs. But both MMU constructors install `Serial::default()`, whose callback is a `noop`
returning `None` ([`serial.rs:3`,`75`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L3)), and **nothing in the repo
ever calls `set_callback`**. So today serial output is silently dropped.

### Honest approximations

- **Transfer is instantaneous.** The full byte is "transferred" the moment `SC` is written
  ([`serial.rs:29`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L29)); there is no per-bit 8192 Hz shifting and no
  transfer-in-progress timing.
- **The start bit is not auto-cleared** after the transfer.
- **No real link partner.** With the default no-op callback, a transfer returns `None`, so
  no interrupt is raised and nothing is received. A harness must install a callback to see
  output.
- **External-clock transfers** (`SC` bit 0 = 0) are ignored (no partner drives the clock).

## 5. Known quirks and edge cases

- **Blargg's serial console.** Blargg's CPU/timing ROMs write their result string to `SB`
  and pulse `SC = 0x81` per character. A working `set_callback` that appends each byte to a
  string reconstructs "Passed"/"Failed" — the foundation of the headless harness
  ([testing](../testing.md)).
- **Disconnected-cable read-back** should shift in `0xFF`; here it depends entirely on the
  callback (default returns nothing).
- **Serial interrupt timing** relative to the 8-bit shift is not modelled (instantaneous).

## 6. Expected behaviours as tests

> **Coverage today: none.**

1. **Callback capture.** After `set_callback`, writing a byte to `SB` then `0x81` to `SC`
   invokes the callback with that byte. *(Not covered — but this is the hook the test
   harness itself relies on.)*
2. **Serial interrupt on transfer.** A completed internal-clock transfer sets `IF` bit 3.
   *(Not covered.)*
3. **`SC` read-back mask.** `rb(0xFF02)` ORs `0x7E`. *(Not covered.)*
4. **Disconnected read.** An internal-clock transfer with no partner shifts in `0xFF`.
   **Expected to FAIL with the default no-op callback.** *(Not covered; known-divergent.)*
5. **Transfer duration.** A DMG internal transfer takes 8 × (1/8192 s); `SC` bit 7 stays
   set until it completes. **Expected to FAIL — instantaneous.** *(Not covered;
   known-divergent.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Blargg (all suites)** | Not testing serial itself — **using** it as the results channel | If the capture callback isn't wired, the harness can't read *any* Blargg result (see [testing](../testing.md)). |
| **Mooneye `acceptance/serial/`** (`boot_sclk_align-dmgABCmgb`) | Serial clock alignment/timing | The instantaneous-transfer approximation is the expected red. |

The declared `retrio/gb-test-roms` submodule is Blargg, whose entire reporting mechanism is
serial — making a working `set_callback` the **first** thing a test harness must add.
**Uninitialised** today. See [testing](../testing.md).

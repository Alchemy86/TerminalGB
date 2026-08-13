# Joypad — input

> Source: [`src/input.rs`](../../src/input.rs) · Related: [MMU](mmu.md) · [Frontends](frontends.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`f333968`](https://github.com/Alchemy86/gameboy/tree/f333968b3a9f0513bc53fb29d52a600562195ca0) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The joypad is the eight-button controller: the four-direction D-pad plus A, B, Start,
Select. The game reads which buttons are held by looking at a single hardware register.
This emulator translates a frontend keypress (a keyboard key, a terminal key event) into
the same register state the real console would present.

## 2. The real hardware

There is **one register, `P1`/`JOYP` (`0xFF00`)**, and it multiplexes all eight buttons
through four wires using a **2×4 matrix** (Pan Docs — [Joypad](https://gbdev.io/pandocs/Joypad_Input.html)):

- Bits 4–5 are **select lines the CPU writes**: bit 4 (`P14`) selects the **direction**
  row, bit 5 (`P15`) selects the **button** row. They are **active-low** (a `0` selects).
- Bits 0–3 are **read-only inputs**, also **active-low**: `0` = pressed, `1` = released.
- Depending on which select line is pulled low, bits 0–3 report either
  Right/Left/Up/Down or A/B/Select/Start.

Pressing a button (a high→low transition on a selected input line) requests the **Joypad
interrupt** (`IF` bit 4) — historically used to wake the console from low-power `STOP`.

## 3. History and versions

The joypad is **identical across DMG, MGB, SGB, and CGB** — same register, same matrix,
same eight buttons.

- **SGB** additionally uses `P1` as a command channel: the SNES multitap and SGB command
  packets are clocked through the select bits. That SGB command protocol is **not**
  implemented here (this emulator has no SGB support).

## 4. How it works here

**The `Keypad` struct** — [`src/input.rs:1`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L1): `row0` (direction
nibble), `row1` (button nibble), `data` (the byte visible at `0xFF00`), and `interrupt`.
Default is all-released, active-low: `row0 = row1 = 0x0F`, `data = 0xFF`
([`input.rs:21`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L21)).

**The `KeypadKey` enum** — [`input.rs:8`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L8), `#[repr(C)]` so the FFI
frontends can pass it across the C ABI.

**The matrix** — `wb` only lets the CPU change bits 4–5
(`data = (data & 0xCF) | (value & 0x30)`, [`input.rs:37`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L37)) and then
`update()` ([`input.rs:42`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L42)) ANDs in `row0` when `P14` is low and
`row1` when `P15` is low. `keydown`/`keyup` clear/set the matching row bit
([`input.rs:78`,`92`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L78)).

**Interrupt** — raised inside `update()` on a high→low transition of the selected nibble:

```rust
if old_values == 0xF && new_values != 0xF {   // input.rs:53
    self.interrupt |= 0x10;                    // IF bit 4
}
```

**Tests** — `keys_buttons` and `keys_direction`
([`input.rs:111`,`140`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L111)) are **2 of the 4 tests in the whole repo**;
they check that each button appears in the right nibble under the right select line.

### Honest approximations

- **Interrupt only fires while the relevant row is selected.** The transition check runs in
  `update()`, so a press is only seen as an edge if the game has that nibble selected at the
  time. Real hardware's line behaviour is subtly more permissive; in practice this is fine
  because games poll continuously.
- **No `STOP`-wake path.** The joypad interrupt is raised, but the CPU's `STOP` handling is
  minimal (see [CPU](cpu.md)), so joypad-wakes-from-STOP isn't a meaningful scenario here.

## 5. Known quirks and edge cases

- **Everything is active-low.** `0` means pressed *and* `0` means selected — a frequent
  source of emulator bugs. Modelled correctly (masks and AND logic in `update()`).
- **Both rows selected at once** (`P14` and `P15` both low): hardware ANDs both rows
  together onto bits 0–3. This emulator ANDs `row0` then `row1` sequentially, matching that
  behaviour ([`input.rs:46–58`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L46)).
- **Neither row selected**: bits 0–3 read as `1` (nothing pressed appears) — the default
  `new_values = 0xF`.
- **Frontend key mapping is inconsistent** between frontends — see [Frontends](frontends.md)
  (the terminal's on-screen help, its actual key handler, and the desktop/web builds
  disagree on which host key is "B").

## 6. Expected behaviours as tests

> **Coverage today: the two matrix tests** (#1/#2); interrupt behaviour untested.

1. **Direction row.** With `P14` selected (`wb(0xFF00, 0x10)`... low bit), pressing Right
   clears bit 0. ✅ **Covered** (`keys_direction`, [`input.rs:140`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L140)).
2. **Button row.** With `P15` selected, pressing A clears bit 0. ✅ **Covered**
   (`keys_buttons`, [`input.rs:111`](https://github.com/Alchemy86/gameboy/blob/main/src/input.rs#L111)).
3. **Active-low released state.** With nothing pressed, `rb(0xFF00)` low nibble reads
   `0xF`. *(Not covered.)*
4. **Only bits 4–5 are writable.** Writing `0xFF` to `0xFF00` leaves bits 0–3 and 6–7
   unchanged from the matrix result. *(Not covered.)*
5. **Joypad interrupt on press.** Pressing a button whose row is selected sets `IF` bit 4.
   *(Not covered.)*
6. **Both rows selected AND together.** With `P14` and `P15` both low, a bit reads `0` if
   pressed in *either* row. *(Not covered.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Mooneye `manual-only/` & `acceptance/` joypad** | Register read-back and interrupt timing | Matrix/active-low logic errors. Joypad tests are limited because input is inherently interactive. |
| *(no Blargg joypad suite)* | — | Input is mostly validated by games booting past their title screens. |

Joypad correctness is largely proven by real games responding to input; the automated
suites focus elsewhere. See [testing](../testing.md).

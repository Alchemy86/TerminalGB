# Super Game Boy (the ICD2 command channel)

> Part of the [component reference](README.md). Source:
> [`src/sgb.rs`](../../src/sgb.rs), wired at
> [`src/mmu/mod.rs`](../../src/mmu/mod.rs) (`$FF00`) and
> [`src/mode.rs`](../../src/mode.rs) (`Model::Sgb` / `Model::Sgb2`).

## What it is, in plain terms

A Super Game Boy is a SNES cartridge with a Game Boy inside it. The Game Boy half is
ordinary DMG hardware; the SNES half draws the picture, adds a decorative border, recolours
the four shades and can play SNES sound. The two halves talk over the **joypad port** — the
only wires the SNES side is allowed to touch — and a game addresses the SNES by pulsing
`P14` and `P15` (bits 4 and 5 of `$FF00`) in a documented pattern.

Almost all of that traffic is **one-way**: a game sends a border, a palette or a sound
request, the SNES acts on it, and the cartridge never learns whether it worked. Exactly one
command answers back, and it answers through `$FF00` itself: **`MLT_REQ`**, which asks the
SNES to read more than one controller and report them in turn.

## What is modelled here — and what deliberately is not

| | |
|---|---|
| ✅ The command-packet receiver on `$FF00` | Every published SGB conformance test is a test *of this*. |
| ✅ `MLT_REQ` and the multiplayer joypad index | The one thing a cartridge can read back. |
| ✅ SGB / SGB2 boot registers | Mooneye `acceptance/boot_regs-sgb`, `-sgb2`. |
| ❌ Border, palettes, `*_TRN` VRAM transfers, SGB sound | **Documented non-goal.** |
| ❌ The SNES side at all (Mooneye's `-S` ROMs) | Would be a second emulated console. |

The line is not arbitrary: **a Game Boy program cannot observe the things we do not
model.** A border is drawn on a screen the cartridge cannot read, so no test ROM can assert
it and this emulator — which shows the 160×144 Game Boy screen — has nowhere to put it. If
SGB video is ever wanted it is a *display* feature, not an accuracy one.

`Model::Auto` therefore **never** boots a Super Game Boy. A cartridge's SGB flag
(`$0146 == 0x03`) says the game *offers* a border and a palette, not that the player owns
the peripheral; booting one on that hint would change the game's boot registers and give it
none of the display it asked for. `GB_MODEL=sgb` (or `sgb2`) asks for it explicitly, which
is also how the GB Emulator Shootout's own manifest drives it.

## The real hardware: the packet protocol

Pan Docs, ["SGB Command Packet
Transfer"](https://gbdev.io/pandocs/SGB_Command_Packet_Transfer.html). A packet is 16 bytes,
LSB first; the low three bits of the first byte say how many packets the command chains
(0 means 1, up to 7). The rest of the first byte is the command id.

The wire rules, as the conformance ROMs pin them down — this is more precise than the prose
documentation, because the tests were written to separate rules the prose leaves ambiguous:

1. Writing `$00` (**both** select lines low) arms a transfer and discards any packet in
   flight.
2. A write of `$30` (both high) while armed **starts** the packet. A bare `$00` that is
   never followed by `$30` starts nothing — the transfer stays dead and every later pulse is
   misread. (`sgb-ext-test`'s `SendPacketShortStart`.)
3. Once started, each write of `$30` latches one bit, and the bit is decided by the value
   **immediately before it**: `$10` (P15 low) is a `1`, `$20` (P14 low) is a `0`. It is the
   *last* line held low that counts, not the first — `sgb-ext-test` sends `$20 → $10 → $30`
   and `$10 → $20 → $30` for the same bit position precisely to tell those two rules apart.
4. The command runs the instant the last bit of the last packet arrives, **not** at the stop
   bit. `sgb-ext-test`'s `SendPacketCorruptStop` ends a packet with a `1` where the stop bit
   should be a `0`, and still expects the command to have taken effect.

## `MLT_REQ`, and the "glitched player 3"

`MLT_REQ` (`$11`) takes a two-bit parameter: `0` = one player, `1` = two, `3` = four. `2` is
not a legal request, and what the hardware does anyway is the interesting part:
`player_count` becomes **3**, so the joypad index wraps with `3 - 1 = 2` and can only ever be
0 or 2 — the machine alternates between player 1 and player *3*.

Three rules, in the order they matter:

* **The index advances on the rising edge of P15** (bit 5 of the value written to `$FF00`),
  and is masked with `player_count - 1` at every step. So a one-player machine never moves,
  and a `$20 → $30` pulse never counts even though it is a perfectly good pulse. That
  asymmetry is the whole of `command_mlt_req_1_incrementing`.
* **Applying `MLT_REQ` advances the index `player_count` more times.** For 1, 2 and 4
  players that is a whole number of turns and is invisible; for the illegal count of 3 it
  leaves the index one step further on than a plain mask would. Physically: the SNES side
  polls every controller it now believes is attached. `command_mlt_req` measures exactly
  this, in sixteen of its twenty-four samples.
* **Reading `$FF00` with both select lines high** returns `$F - index` in the low nibble
  when more than one player is configured, and the usual `$F` otherwise. With a row
  selected, ports 2-4 read as nothing pressed — this emulator has one pad, and a port with no
  pad plugged in is what hardware reports.

### Where other emulators get this wrong

Worth recording, because it is why these rows are red almost everywhere:

* **SameBoy** rounds the illegal three-player request up to four
  (`if (player_count == 3) player_count++;` in `Core/sgb.c`). That single line is why SameBoy
  — the reference implementation for SameSuite, whose author wrote the test — **fails
  `samesuite/sgb/command_mlt_req`**.
* Of the eighteen emulators in the published GB Emulator Shootout, **only GSE passes all
  three SGB rows**; SameBoy passes one, bgb one, mGBA two, and six decline the SGB model
  entirely (which drops the rows from their denominator — see below).

## How it works here

* [`src/sgb.rs`](../../src/sgb.rs) — the whole of it: a select-line state machine, a
  128-bit-per-packet buffer and the joypad index. No allocation, no hot-path cost.
* [`src/mmu/mod.rs`](../../src/mmu/mod.rs) — `sgb: Option<Sgb>`, created by `set_mode` only
  when the mode is `Super`/`Super2`. Every write to `$FF00` is forwarded to it (a packet bit
  and a joypad poll are the same two wires); reads go through `joypad_read`, which overlays
  the player nibble. A machine that is not an SGB pays one `Option` test per `$FF00` access.
* [`src/mode.rs`](../../src/mode.rs) — `GbMode::Super` / `Super2`, and `GbMode::is_dmg()`,
  which is what every "this is DMG hardware" branch in the PPU, MMU and CPU now asks. The
  two SGB revisions differ **only** in the boot value of `A` (`$01` vs `$FF`); everything
  else is `F=$00, BC=$0014, DE=$0000, HL=$C060` (Pan Docs, "Console state after boot ROM").

### Honest approximations

* **Only `MLT_REQ` is acted on.** Every other command is parsed, checked for length, and
  dropped. That is what a Game Boy with no SGB attached does, and no test ROM can tell.
* **One controller.** Players 2-4 always read as nothing pressed. Nothing in the frontends
  offers a second pad yet; when one does, `Sgb::player_index` is the hook.
* **No joypad interrupt from an index change.** On hardware the low nibble changes as the
  index advances, which can raise the joypad interrupt. No test measures it and no game
  relies on it.
* **The SGB's boot system-counter value and `P1` state are assumed to be a DMG's.** Mooneye's
  `boot_div-S` would measure the first of those; it is one of the `-S` ROMs, which need the
  SNES side.

## Expected behaviours, phrased as tests

1. *A packet whose opening `$00` is not acknowledged by a `$30` is ignored.* — **covered**
   (`a_transfer_without_its_opening_pulse_is_ignored`, and `sgb-ext-test`).
2. *The bit is the last line held low before the lines go high.* — **covered**
   (`the_last_line_held_low_decides_the_bit`, and `sgb-ext-test`'s `20To10`/`10To20`).
3. *A corrupt stop bit does not cancel a completed command.* — **covered**
   (`a_corrupt_stop_bit_does_not_cancel_the_command`).
4. *The player index advances on a rising P15 edge and on nothing else.* — **covered**
   (`only_a_rising_p15_edge_advances_the_player`, eight pulse patterns from
   `command_mlt_req_1_incrementing`).
5. *`MLT_REQ 2` is three players, not four.* — **covered**
   (`the_illegal_three_player_request_alternates_players_one_and_three`).
6. *A `$FF00` read reports the player index only with both select lines high.* — **covered**
   (`a_super_game_boy_reports_the_player_through_the_joypad_register`, in `src/mmu`).
7. *An overlong packet count cannot index past the buffer.* — **covered**
   (`an_overlong_command_cannot_run_off_the_buffer`); the packet count is a cartridge-chosen
   byte, so this is a crash-safety check, not an accuracy one.
8. *A save state carries the ICD2.* — **covered**
   (`state_survives_a_save_and_load`; `savestate::FORMAT_VERSION` is 12).
9. *A border, a palette or an `ATTR_*` command changes the picture.* — **not covered, and
   not intended to be.** See the non-goals above.

## The ROMs that exercise it

| ROM | Runner | Verdict |
|---|---|---|
| `samesuite/sgb/command_mlt_req` | `examples/samesuite.rs` (the ROM's own `RESULT_CODE`) | **PASS** |
| `samesuite/sgb/command_mlt_req_1_incrementing` | same | **PASS** |
| `cpp/sgb-ext-test` | `examples/cpp_suite.rs` (the author's reference PNG) | **PASS**, 0/23,040 pixels differ |
| `acceptance/boot_regs-sgb`, `-sgb2` | `examples/mooneye_scout.rs` | **PASS** |

All four are gated by `make test-external` and CI. The two SameSuite rows are also
pixel-exact against the shootout's reference screenshots — which needed one thing that is
*not* SGB behaviour at all, recorded below because it is the kind of gap that hides.

### The boot ROM's leftover tilemap

Both SameSuite `sgb/` ROMs write only their results table and never clear the tilemap, so
tile rows 8 and 9 still hold whatever the boot ROM left there: the Nintendo logo's map
entries, `$01`-`$0C` across `$9904`-`$990F`, `$0D`-`$18` across `$9924`-`$992F`, and `$19`
(the ® glyph) at `$9910`. On hardware those cells render as whatever the *game* has since
loaded at those tile ids — for these ROMs, its hex font. This emulator runs no boot ROM and
booted with a blank tilemap, so those two rows were empty and **every white pixel on the
screen ranked differently**, i.e. a whole-screen mismatch on two tests whose actual subject
was fine.

`MemoryManagementUnit::set_boot_logo_tilemap` now writes those entries for every DMG-class
machine. The tile *pixels* are not written: on hardware they are the logo the boot ROM
decompressed out of the cartridge header, and every ROM that reaches `$0100` has replaced
them long before anything looks. Pinned by
`the_boot_rom_leaves_its_logo_in_the_tilemap`.

## What declaring SGB support is worth

The shootout scores an emulator out of the rows it can run. Six of the eighteen listed
emulators — including **docboy, which is ranked first at 258/261** — decline the SGB model,
and their three SGB rows simply vanish from their denominator. Declining was the cheap
option here too, and it was the recommendation in the audit that commissioned this work.
Implementing it instead is worth **three published rows out of 264** plus two Mooneye rows
outside that set, and it costs one file, one `Option` on the `$FF00` path and no hot-path
work at all.

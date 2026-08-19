# Picture modes — `standard` and `identical`

> **Key:** `P` switches mode live. [Every control →](controls.md)

TerminalGB carries **two complete picture engines**, and which one runs is a mode you choose.

| | **`standard`** | **`identical`** |
|---|---|---|
| The default | **yes** | no — opt in |
| How it draws | a whole scanline at once, at the mode 3 → 0 transition, from whatever the registers hold at that instant | the real hardware pixel pipeline, dot by dot: BG fetcher, 8-pixel FIFO, sprite fetches, window restart, `SCX & 7` discard |
| Mode 3 length | fixed at 172 dots | variable, as it falls out of the machine (172–289 on hardware) |
| A register written mid-scanline | invisible — only its final value is read | lands exactly where a Game Boy puts it |
| Cost per frame (dmg-acid2) | **0.221 ms** (76× real time) | **0.461 ms** (36× real time) — about **2.1×** |
| Almost every game | identical output, proved frame-for-frame | identical output |

They are **not** a quality or appearance choice. Palette, upscaler, transport and every other
visual setting live above the PPU in `src/screen/` and are byte-identical either way. The only axis
is *exactness against speed*.

> **The old names still work everywhere.** `standard` was `fast` and `identical` was `accurate`,
> and both spellings parse in every place a mode is named — `config.toml`, `$GB_PPU`, the `make`
> targets, `RenderMode::parse`. Nothing anybody has written stops working.

---

## 1. How to choose one

**In the terminal frontend, press `P`.** The mode switches live — no pause, no reload, no restart —
because the two engines share every register and the framebuffer, and `identical` rebuilds its
pipeline at the head of the next scanline. The Status panel's `PPU` row says which one is live and,
for `identical`, what it costs:

<p align="center">
  <img src="shots/tui-status-picture-mode.png" alt="The terminal Status panel: the PPU row reads &quot;identical ~2.1x&quot;" width="260" />
</p>

Or set it before launch, in `${XDG_CONFIG_HOME:-~/.config}/terminalgb/config.toml`:

```toml
ppu = identical      # or: standard (the default), accurate, fast
```

Or for one run:

```sh
GB_PPU=identical  cargo run --release --example terminal -- game.gb
GB_PPU=standard   cargo run --release --example terminal -- game.gb   # the default
```

`$GB_PPU` wins over the config key, so a one-off override is not silently undone by a config file.
A misspelling **warns and is ignored** rather than quietly switching mode — the same rule
`$GB_MODEL` follows, and for the same reason: a conformance score published from the mode nobody
asked for is worse than no score.

Embedding? [`Gameboy::set_render_mode`](../src/gameboy.rs) takes
[`terminalgb::ppu::RenderMode`](../src/gpu/mod.rs) and is safe to call at any point.

**The mode is not part of a save state.** It is a property of the machine *you* are running, not of
the cartridge — loading somebody else's state must not switch your renderer underneath you. Same
call as the console revision and the CGB APU revision.

---

## 2. When `identical` is worth it

Reach for it when:

- **a game has a mid-scanline effect that looks wrong** — a split status bar, a screen wobble, a
  parallax band, a shrinking window. These are made by writing `BGP`, `SCX`, `SCY`, `LCDC` or `WX`
  part-way along a line, which `standard` cannot see;
- **you are working on the PPU**, or scoring the picture suites;
- **you have CPU to spare.** At 36× real time on an ordinary desktop, `identical` still has an
  order of magnitude of headroom over the 59.7275 Hz a Game Boy runs at.

Stay on `standard` when you want the cheapest correct picture — which, for the overwhelming
majority of cartridges, is the *same* picture. That is not an assumption:
[`testharness/ppuengines.rs`](../testharness/ppuengines.rs) renders 600 frames of dmg-acid2 and of
Blargg's `cpu_instrs` through both engines every `cargo test --release` and requires the
framebuffers to be **byte-identical**.

---

## 3. What each mode scores

Both modes are gated in CI, each against **its own baselines**, so neither can drift unnoticed:
`make test-external` scores `standard`, `make test-external-accurate` scores `identical`.

| Gated suite | `standard` | `identical` |
|---|---:|---:|
| Mooneye GB `acceptance` | 67 / 75 | **69 / 75** |
| Mealybug Tearoom — pixel-exact rows | 3 / 79 | **17 / 79** |
| Mealybug Tearoom — pixels wrong (lower is better) | 1,212,282 | **630,266** |
| dmg-acid2 against its author's reference | 0 / 23,040 differ | 0 / 23,040 differ |

Everything else — the in-repo Blargg harness (46/47), Mooneye `emulator-only` (28/28) and `misc`
(8/8), SameSuite (72/76), CasualPokePlayer (4/4) — measures the CPU, the APU, DMA or the cartridge
mappers, which the picture mode does not reach. Those suites have one baseline each, and the
[status page](status.md) says `same` for them rather than repeating a number nobody re-measured.

**`identical` now loses no row to the default on any gate.** It used to be behind on two
(`ppu/intr_1_2_timing-GS` and Blargg `oam_bug` sub-test 7); both are closed. What stands between it
and being the default is only its cost.

**Read the Mealybug pixel distance, not just the row count.** It is a *shade-rank* distance, so a
frame with one extra distinct shade reports ~23,040 whatever else is right, and a four-pixel error
can move the total by tens of thousands. The row count is a wall at this range; the distance is the
thing that shows a pipeline change getting closer. Both are in the baselines.

---

## 4. The cost figure, and how it stays honest

`RenderMode::IDENTICAL_COST` is **7.6**, and it is measured rather than chosen. **It has moved
four times, and the fourth is the first time it moved because `identical` got FASTER.** First
2.1 → 6.3, when the PSP work rewrote the whole-scanline renderer to fetch once per tile instead
of once per pixel; then 6.3 → 6.8, when the shipping profile stopped optimising for size; then
6.8 → 8.8, when `Gpu::blit_dmg_tile` started drawing a whole monochrome tile with word
arithmetic. All three were `standard` — a branchy whole-scanline loop is exactly the shape an
optimiser, or a human, can do something with, while the per-dot engine's pixels come out of the
fetcher.

8.8 → 7.6 is the other direction: the per-dot engine gained the idle batch it had never been
able to use and stopped walking mode 2 a dot at a time
([`docs/throughput.md`](throughput.md) §14). Measured in one session, same harness, HEAD against
the change: **8.44× → 7.25/7.31/7.32×** on dmg-acid2 (accurate 0.6759 → 0.5653 ms/frame), and
on a real cartridge — Pokémon Blue from an overworld save state, where the CPU actually halts —
**0.5755 → 0.332 ms/frame**. The constant is the published 8.8 scaled by the ratio HEAD and the
change measured against each other in that session, because both ends are depressed by the same
contention.

[`testharness/ppuengines.rs`](../testharness/ppuengines.rs) times both engines over the same 400
frames of dmg-acid2 on every `cargo test --release` and **fails the build if the constant disagrees
with what it measured**. Twelve interleaved runs on the machine this was last measured on gave
**7.03–10.00×, median 8.5** (fast 0.056–0.078 ms/frame, accurate 0.545–0.666); the *same harness on
the parent commit, in the same session*, gave **5.57–6.38× against its published 6.8**, so both
ends were depressed by the same contention and the new constant is the median scaled onto the old
one. An earlier low-load session gave 6.72–6.78 where a busy box gave 7.75 and a very busy one
5.56, which is why the figure is shown with a `~` and checked against a wide band.

That check exists because this repository has been bitten by a stale performance number before —
"audio costs ~8 ms a frame" outlived the fix by months and kept being quoted. A figure printed on a
status row, in a toast and in the help overlay is the whole basis on which somebody decides to trade
speed for exactness; it has to be a measurement, not folklore.

Reproduce it:

```sh
cargo test --release --test ppuengines
```

---

## 5. Where the modes are named in the product

An option nobody can find is an option nobody has, so the mode appears in five places and they are
kept consistent by tests rather than by care:

- the **`P` key** in the terminal frontend, with a toast naming the new mode and its cost;
- the **`PPU` row** of the Status panel, always visible;
- the **Controls list** (`P picture mode`) and the **`?` help overlay**;
- the **`ppu`** config key, whose warning offers every spelling back;
- the header of **every baseline file**, which records the mode that scored it and the `make`
  target that re-blesses it — a baseline that did not say would be a file nobody could safely
  re-bless.

---

## 6. Further reading

- [`docs/mealybug.md`](mealybug.md) — what the per-dot engine still gets wrong, row by row, and what
  closing each would take. Read it before touching `src/gpu/fifo.rs`.
- [`docs/components/ppu.md`](components/ppu.md) — the component page: registers, modes, the
  interrupt line, and the honest approximations of each engine.
- [`docs/measured/ppu.md`](measured/ppu.md) — the measured picture unit: what the references say,
  what we measured, and what a wrong emulator does.
- [`docs/testing.md`](testing.md) — how the suites are fetched, scored and gated.

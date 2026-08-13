# Telemetry — machine-readable run data (NDJSON)

> The captain: *"as with our whole desire to have things plug and play with our player — show
> stats, framerate etc — it should be the case that these details are also visible to something
> like YOU when running. So ensure this is the case, so testing should be a breeze with our own
> built-in features and data points."*

Everything the on-screen stats overlay shows a human, this stream shows a program. It exists
because an agent (or a CI job) cannot see the screen or hear the speaker, so "did that change
actually help?" was previously unanswerable without a person watching. Now it is one command.

Implementation: [`src/telemetry.rs`](../src/telemetry.rs). Emitted by the terminal frontend
([`src/screen/tui.rs`](../src/screen/tui.rs)) and by `examples/render_bench.rs`.

This is the first real implementation of the **emit** half of the extension surface designed in
[`debugging.md`](debugging.md#the-public-extension-surface--emit-control-transport) — the
`Frame rendered` event, the one that design calls *"always cheap — the general capture
primitive"*. It follows that design's rules: opt-in, serialisable messages rather than a
transport-specific API, and **zero cost when nothing is subscribed**.

---

## 1. Turning it on

Off by default. Nothing is measured, formatted or written unless a destination is configured;
disabled, the whole mechanism is one `Option` test per displayed frame.

| Where | Value | Meaning |
|---|---|---|
| `GB_TELEMETRY` env | `/path/to/file.ndjson` | append NDJSON to that file |
| `GB_TELEMETRY` env | `-` or `stdout` | write to stdout (**headless only** — see below) |
| `GB_TELEMETRY` env | `off`, `false`, `0`, empty | disabled |
| `GB_TELEMETRY_EVERY` env | `n` | emit one record every *n* frames (default `1`) |
| `config.toml` | `telemetry = "/tmp/gb.ndjson"` | same, lower precedence than the env var |

The environment variable wins over the config key.

**stdout is refused by the terminal frontend.** It runs inside the alternate screen, so writing
JSON to stdout would corrupt the display; asking for it produces a loud config warning and
telemetry stays off. Give it a file path. `stdout` is for the headless tools.

Sampling with `GB_TELEMETRY_EVERY` reduces the *written* records only — the run summary still
accounts for every frame.

---

## 2. One-shot mode: run N frames, print one JSON object, exit

This is the form that makes a change verifiable in a test or in CI, with no terminal and no
human:

```console
$ cargo run --release --example render_bench -- \
      --rom tests/cpu_instrs/cpu_instrs.gb --frames 200 --motion \
      --cols 320 --rows 90 --upscale nearest --factor 1 --path capped --json
{"schema":1,"type":"summary","frontend":"bench/capped","rom":"CPU_INSTRS",…,
 "frames":200,"fps_avg":1776.268,"emulated_fps_avg":59.728,"speed_pct_avg":100.000,
 "frame_ms_avg":0.566,"encode_ms_avg":0.212,"tx_bytes_avg":93430.000,"tx_mb_per_s":5.322,…}
```

Add `--ndjson` instead of `--json` for one record per frame. Drop `--cols/--rows/--upscale`
and it sweeps a matrix of window sizes and upscaler settings and prints a human table.

---

## 3. Schema

**A stable contract.** Consumers must ignore unknown fields. New fields may be added at any
time; existing fields are not renamed, retyped or removed without bumping `schema`
([`SCHEMA_VERSION`](../src/telemetry.rs)). Every record carries the full context, so a consumer
that only ever sees one line still knows what it is looking at.

All durations are **milliseconds**, written with three decimals. Non-finite values are written
as `0.000` — the output is always valid JSON.

### 3.1 Common to every record

| Field | Type | Meaning |
|---|---|---|
| `schema` | int | schema version (currently `1`) |
| `type` | string | `"frame"` or `"summary"` |
| `frontend` | string | `"terminal"`, `"bench/capped"`, `"bench/legacy"` |
| `rom` | string | cartridge header title |
| `mbc` | string | memory-bank-controller name |
| `protocol` | string | `kitty` / `iterm2` / `sixel` / `halfblocks` |
| `palette` | string | `grayscale` / `green` / `amber` |
| `upscale` | string | `nearest` / `scale2x` / `mmpx` / `lcd` |
| `upscale_factor` | int | the factor the **user** selected |
| `speed` | string | speed rung: `0.5x`, `1x`, `2x`, `4x`, `MAX` |
| `scale` | float | on-screen magnification of the 160×144 picture |
| `term_cols`, `term_rows` | int | terminal size in cells (`0` when not known) |
| `display_cols`, `display_rows` | int | the picture's rectangle, in cells |
| `tx_w`, `tx_h` | int | **pixel size actually transmitted** to the terminal |
| `display_px_w`, `display_px_h` | int | pixel size the terminal stretches it to |

`tx_w`×`tx_h` versus `display_px_w`×`display_px_h` is the whole performance story: the first is
what costs time, the second is what the user sees. They used to be the same number.

### 3.2 `type: "frame"` — one per displayed frame

| Field | Type | Meaning |
|---|---|---|
| `frame` | int | emulated-frame counter since the session started |
| `fps` | float | **displayed** frames per second, averaged over the last second |
| `emulated_fps` | float | **emulated** frames per second — must stay at 59.7275 |
| `speed_pct` | float | `emulated_fps / 59.7275 × 100` |
| `frame_ms` | float | wall-clock cost of the draw |
| `emulate_ms` | float | one emulated frame |
| `palette_ms` | float | palette mapping |
| `upscale_ms` | float | pixel-art upscaler |
| `encode_ms` | float | protocol encode (base64 / PNG container) |
| `write_ms` | float | the rest of the draw: `ratatui` diff + bytes to the terminal |
| `frames_skipped` | int | emulated frames since the previous displayed one |
| `frames_identical` | int | cumulative frames whose picture was unchanged, so nothing was encoded or sent |
| `frames_dropped` | int | cumulative emulated frames never displayed (render behind) |
| `tx_bytes` | int | escape-sequence bytes for this frame (`0` = unchanged frame) |
| `audio` | object or `null` | `null` when no host stream is attached |

`audio` object: `queue_depth`, `underruns`, `dropped`, `callbacks`, `sample_rate`. Read from
the player's existing diagnostic handle
([`CpalPlayer::queue_handle`](../src/sound.rs)) with a **non-blocking** `try_lock` — telemetry
never stalls the render loop, and a contended lock simply reports nothing that frame.

### 3.3 `type: "summary"` — one per run

`frames`, then `fps_avg`, `emulated_fps_avg`, `speed_pct_avg`, `frame_ms_avg`,
`frame_ms_worst`, `emulate_ms_avg`, `palette_ms_avg`, `upscale_ms_avg`, `encode_ms_avg`,
`write_ms_avg`, `tx_bytes_avg`, `tx_mb_per_s` (bytes/frame × 59.7275), plus the cumulative
`frames_identical`, `frames_dropped`, `audio_underruns` and `write_errors`.

`tx_mb_per_s` is the number to watch when choosing `transmit_scale`: it is what the terminal's
parser has to swallow, and it is the one cost that a faster encoder cannot remove.

---

## 4. Reading it

```console
# watch a live session
$ GB_TELEMETRY=/tmp/gb.ndjson cargo run --release --example terminal -- game.gb
$ tail -f /tmp/gb.ndjson | jq -c '{fps, emulated_fps, encode_ms, tx_bytes}'

# is the game running at true speed?
$ jq -s 'map(.speed_pct) | add/length' /tmp/gb.ndjson

# what dominates the frame?
$ jq -s 'map({emu:.emulate_ms, pal:.palette_ms, up:.upscale_ms, enc:.encode_ms, wr:.write_ms})
         | (map(.enc)|add/length)' /tmp/gb.ndjson
```

The invariant worth asserting in any automated check: **`speed_pct` stays at 100 regardless of
`fps`.** A falling `fps` with `speed_pct` at 100 means the display is dropping pictures, which
is correct behaviour. `speed_pct` below 100 means the emulator itself is late — that is a bug,
and it would also break audio, which is paced from the same clock.

---

## 5. Cost

* **Disabled** (the default): one `Option` test per displayed frame. The stage timers are
  `Instant::now()` pairs that the on-screen stats overlay wants anyway.
* **Enabled**: one record is ~700 bytes formatted into a reused `String` and written to a
  `BufWriter`. At 60 Hz that is ~42 KB/s. Measured against the same run with telemetry off, it
  does not move `frame_ms_avg` out of its noise band.
* Telemetry never fails a run: an unopenable destination degrades to disabled with a warning,
  and write errors are counted (`write_errors`) rather than propagated.

---

## 6. What is deliberately not here yet

The [debugging design](debugging.md) lists the rest of the emit surface — serial bytes, MBC
bank switches, APU triggers, interrupts, and the two hot events (memory access, instruction
retired) that must stay behind the `debug` cargo feature and a ring buffer. None of them are
implemented; this page covers the frame event only. The **control** and **socket transport**
halves are likewise still design.

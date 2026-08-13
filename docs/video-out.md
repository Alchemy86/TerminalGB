# Video out — the picture as a stream, and as a webcam

> The captain: *"can we confirm it's also possible for our terminal to PROVIDE A VIDEO STREAM
> OUT for things to plumb into and play, or even a webcam to PICK IT UP DIRECTLY!! like a
> camera feed!"*

**Yes to both.** The frame stream is in the emulator ([`src/video.rs`](../src/video.rs)); the
webcam is one `ffmpeg` process away, because `v4l2loopback` is a kernel module that needs root
to install and we will not ship untested `ioctl` code to avoid one extra process. Both routes,
the exact commands, and what was actually captured as proof, are below.

This is the **pixels** half of the emit surface in [`docs/debugging.md`](debugging.md).
[`docs/telemetry.md`](telemetry.md) publishes the *numbers* of every frame as NDJSON; this
publishes the frame.

---

## 1. The one-line version

```bash
# record what a headless run draws, straight into an MP4
cargo run --release --example video_out -- --rom game.gb --frames 600 \
  | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p gb.mp4

# stream a game you are PLAYING in the terminal, through a FIFO
mkfifo /tmp/gb.raw
ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i /tmp/gb.raw -pix_fmt yuv420p gb.mp4 &
cargo run --release --example terminal -- game.gb --video-out /tmp/gb.raw

# a game you are playing, in a window, straight down a pipe
cargo run --release --example desktop -- game.gb --video-out - \
  | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p gb.mp4
```

Nothing to remember: the emulator prints the exact consuming command for whatever settings are
in force, on startup and into the `?` overlay's notices.

```
video: 640x480 yuyv422 (display, realtime) -> /tmp/gb.raw | consume with: ffmpeg -f rawvideo
  -pix_fmt yuyv422 -s 640x480 -framerate 59.7275 -i /tmp/gb.raw -pix_fmt yuv420p out.mp4
```

## 2. Settings

Off by default. Every setting has an environment variable, a config key and (on the two example
binaries) a command-line flag. **Flag beats environment beats config file** — the flags are
implemented *as* environment overrides, so there is no third precedence rule to remember.

| What | Flag | Env | `config.toml` | Values |
|---|---|---|---|---|
| destination | `--video-out` | `GB_VIDEO_OUT` | `video_out` | `-`/`stdout`, a file, a FIFO, a directory, `off` |
| pixel format | `--video-format` | `GB_VIDEO_FORMAT` | `video_format` | `rgba` (default), `rgb24`, `yuyv` |
| frame size | `--video-size` | `GB_VIDEO_SIZE` | `video_size` | `WxH`, e.g. `640x480` |
| picture source | `--video-source` | `GB_VIDEO_SOURCE` | `video_source` | `native` (default), `display` |
| pacing | `--video-pace` | `GB_VIDEO_PACE` | `video_pace` | `realtime` (default), `emulated` |
| recordings dir | — | — | `video_dir` | default `$XDG_VIDEOS_DIR/gameboy`, else `~/Videos/gameboy` |
| camera to verify | — | — | `video_device` | a `/dev/videoN` loopback node (label check only) |

In the terminal frontend, **`v` starts and stops the stream while the game runs** — see §6.

### Native or upscaled? Both, selectable

* `video_source = native` (**the default**) — the raw 160×144 framebuffer. The honest source:
  no palette, no upscaler, exactly what the PPU produced, and the consumer scales it however it
  likes. This is the right default for a *source* feed.
* `video_source = display` — palette-mapped and run through the pixel-art upscaler, i.e. what
  the player is looking at, carrying the green/amber palettes and the LCD/MMPX look.

`display` follows the live keys: `[` (palette) and `]`/`}` (upscaler) change the stream as they
change the screen. Its geometry is fixed when the stream opens (see §4), so cycling to a
different upscale factor mid-stream is letterboxed into the size the consumer was told, never
resized underneath it.

### Which format?

| Format | Bytes/px | Use it for |
|---|---|---|
| `rgba` | 4 | the default. Zero conversion — it *is* the framebuffer. ffmpeg is happy. |
| `rgb24` | 3 | 25 % fewer bytes, still trivially converted, what most encoders want. |
| `yuyv` | 2 | packed 4:2:2, **BT.601 limited range** — what a real UVC webcam emits, and the format `v4l2loopback` consumers accept most widely. Use this for the camera route. |

`yuyv` needs an even width; an odd one is rounded up with a complaint rather than emitting a
stream every consumer mis-parses by one column per row.

## 3. Pacing — what happens when you pause or fast-forward

`rawvideo` has no timestamps. Frame *number* is the clock, so what the stream emits per second
of wall time is the whole contract.

* **`realtime` (default).** One frame per 1/59.7275 s of wall clock, whatever the emulator is
  doing. **Fast-forward drops frames** rather than producing a 4× speed video — a 60-second
  recording is 60 seconds long and the game in it moves at the speed the player saw. **Pause and
  slow motion repeat the last frame**, so the pipe never stalls and a recording made across a
  pause is not short by the length of the pause. A long stall (suspend, a modal overlay) resyncs
  instead of dumping a burst; each resync is counted.
* **`emulated`.** Exactly one output frame per emulated frame — no duplication, no dropping.
  Fast-forward then *does* produce a fast video, and a headless run finishes as fast as the CPU
  allows while still playing back at 59.7275 fps. Deterministic, so it is usable as a fixture.
  `examples/video_out.rs` defaults to this.

**Display frames are dropped; streamed frames are not.** The stream is fed from
`App::step_frame` — the *emulated* frame — not from the draw. The terminal deliberately drops
display frames on a large window ([`docs/terminal-render.md`](terminal-render.md)); a capture
must not lose those frames with it. `the_video_stream_is_fed_by_emulated_frames_not_displayed_ones`
pins that.

## 4. Geometry is fixed for the life of the stream

The consumer is told `-s WxH` once. So the output size is decided when the stream opens and
never changes; a frame of any other size is **letterboxed** into it:

* integer magnification whenever it fits (160×144 → 480×432 inside 640×480), because pixel art
  at a fractional scale gets uneven pixel widths and shimmers;
* the remainder is opaque black bars — **letterboxed, never stretched**. 160×144 is 10:9 and
  every conventional camera size is 4:3 or 16:9, so bars are unavoidable and stretching would
  be worse.

`160x144` is an unusual camera resolution and some consumers reject it, which is why
`--video-size 640x480` exists and is what `tools/virtual-camera.sh` uses.

## 5. The virtual webcam

### Status on this box: **not available, and here is exactly why**

`v4l2loopback` is not installed here — `/dev/video0` and `/dev/video1` are a real UVC webcam,
`lsmod | grep v4l2loopback` finds nothing, and `modinfo v4l2loopback` reports
`Module not found`. Installing it means a kernel module, which means root, which is not this
crew's to do. **So the virtual camera has not been demonstrated end to end.** What *has* been
demonstrated is the whole stream up to the device: the exact YUYV 640×480 bytes the loopback
device would receive, accepted and decoded by `ffmpeg` (§7).

### The route that works

```bash
tools/virtual-camera.sh --name "POKEMON BLUE" --size 640x480
```

It detects the module, finds (or explains how to create) a loopback node, checks the label, and
then runs the bridge. Without the module it prints the install commands and exits 2 rather than
failing obscurely. By hand:

```bash
# once, needs root
sudo dnf install akmod-v4l2loopback v4l2loopback-utils     # Fedora
sudo apt install v4l2loopback-dkms v4l-utils               # Debian/Ubuntu
sudo pacman -S v4l2loopback-dkms v4l-utils                 # Arch
sudo modprobe v4l2loopback video_nr=10 card_label="POKEMON BLUE" exclusive_caps=1

# the bridge
mkfifo /tmp/gb.raw
ffmpeg -f rawvideo -pix_fmt yuyv422 -s 640x480 -framerate 59.7275 -i /tmp/gb.raw \
       -f v4l2 -pix_fmt yuyv422 /dev/video10

# the emulator
cargo run --release --example terminal -- game.gb \
    --video-out /tmp/gb.raw --video-format yuyv --video-size 640x480
```

`exclusive_caps=1` is what makes Chrome, Firefox and Zoom accept the device (without it the node
advertises both capture and output and some consumers skip it).

### Why the emulator does not write to `/dev/videoN` itself

A V4L2 output device needs its format negotiated with `VIDIOC_S_FMT` before the first `write()`.
That is not hard — but it cannot be *tested* without the kernel module, which needs root to
install, and hand-written `ioctl` bindings that no test has ever exercised are worse than no
code at all. Pointing `video_out` at a `/dev/video*` node is therefore refused with the exact
recipe above rather than failing with `EINVAL` halfway through a stream. One extra process buys
format conversion, scaling and MJPEG for free.

### Naming the camera after the game

"POKEMON BLUE" in a camera picker beats "Dummy video device (0x0000)". The label comes from the
`card_label` module parameter at load time, or from `v4l2loopback-ctl add -n`:

```bash
sudo v4l2loopback-ctl add -n "POKEMON BLUE" /dev/video10
```

**A v4l2loopback device cannot be renamed while it exists.** The upstream README documents
`add`, `delete`, `set-fps`, `set-caps` and `set-timeout-image` — and no rename; the only way to
change a label is to delete and re-create the device, which needs root and drops every consumer
attached to it. So the emulator does the one useful thing it *can* do: set `video_device =
"/dev/video10"` in `config.toml` and it reads the device's label from sysfs, checks it against
the running cartridge, and tells you the two commands to fix it if they disagree:

```
video: /dev/video10 is labelled "TETRIS" but you are playing "POKEMON BLUE" — a v4l2loopback
device cannot be renamed while it exists, so re-create it (needs root, drops consumers):
  sudo v4l2loopback-ctl delete /dev/video10
  sudo v4l2loopback-ctl add -n "POKEMON BLUE" /dev/video10
```

The match is loose on purpose (alphanumerics, case-insensitive, containment) so
`"Game Boy: Pokemon Blue"` passes — the point is to catch *the wrong game*, not to police
formatting.

### For the pipe route, the equivalent is naming the file

* a `video_out` **directory** (or no `video_out` at all, with `v`) writes
  `POKEMON_BLUE-20260813-142530-640x480-yuyv422.raw` into it — the game, when, and **the two
  things `rawvideo` cannot carry in-band**, so the consuming command can be read off the name;
* `%t` and `%s` placeholders work anywhere in the path: `video_out = "~/Videos/%t-%s.raw"`;
* titles are cleaned first — header titles are 16 bytes of upper-case ASCII, NUL- or
  space-padded, and may hold anything at all, so `clean_title` keeps printable ASCII and
  collapses padding for a device label, and `file_safe_title` maps everything non-alphanumeric
  to `_` for a filename (`../../etc/passwd` → `etc_passwd`).

An RTMP stream key is the same idea one process further out:
`… | ffmpeg -i - -f flv rtmp://host/live/$(basename gb-POKEMON_BLUE)`.

## 6. Toggling it live (terminal frontend)

`v` arms and disarms the stream mid-game — no restart.

* With `video_out` configured it simply starts/stops that destination.
* With nothing configured it records to a file named after the game in `video_dir`.
* The **Status panel** carries a `Video` row: `off`, or `● LIVE 12M`. You are never left
  guessing whether you are live.
* A toast confirms both directions (`● LIVE → /tmp/gb.raw`, `Video stream stopped — 3600
  frames, 316.4 MB`), and the full `ffmpeg` command goes into the `?` overlay's notices.
* Loading a different ROM re-opens a *game-named* stream under the new name; a FIFO or file you
  chose yourself keeps running across ROM switches, because its consumer is attached to that
  path.

Raw video is **5.5 MB/s at 160×144 RGBA** and 36 MB/s at 640×480 YUYV. The Status row shows the
total so a recording left running is visible rather than a surprise.

**On the desktop frontend there is no toggle** (it has no toast or status panel to report
through) — configure it at launch. Note its long-standing behaviour that an **unfocused window
pauses emulation**: the stream then holds the last frame at the wall-clock rate rather than
stalling, which is the right answer for a capture but means an unfocused window records a still
picture. Click the window to keep it live.

## 7. What was actually captured (proof)

Run on this box, with the vendored MIT `dmg-acid2` ROM:

| Command | Result |
|---|---|
| `video_out --frames 300 \| ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p acid2.mp4` | 160×144, **300 frames, 23891/400 = 59.7275 fps**, 137 KB. Extracted frame 300 is the acid2 face, "HELLO WORLD!" and all. |
| `video_out --frames 240 --format yuyv --size 640x480 --source display --palette green --upscale lcd --factor 3 \| ffmpeg -f rawvideo -pix_fmt yuyv422 …` | 640×480, 240 frames, 59.7275 fps. Decoded frame: green palette, LCD grid, **pillar/letterboxed with black bars, not stretched**. |
| `video_out --frames 120 --pace realtime --out /tmp/gb.raw` with `ffmpeg` reading the FIFO | 120 frames, **duration 2.009 s** (120 / 59.7275 = 2.009). 0 duplicated, 0 dropped. |
| `video_out --out /dev/video10` | Refused, with the `mkfifo` + `-f v4l2` recipe and the install commands. |
| `tools/virtual-camera.sh --name "POKEMON BLUE"` | Detected the missing module, printed the install + `card_label="POKEMON BLUE"` commands, exit 2. |

Artefacts land in `target/video-proof/` (gitignored). Reproduce all of it with the commands
above; nothing here needs a terminal, a window or a human.

## 8. Cost

**Off by default, and free when off:** `video_tick` returns on a single `Option` test and does
not even read the clock. With it on, measured over 3000 frames of `dmg-acid2` (emulation alone
is 0.216 ms/frame on this box):

| Stream | ms/frame total | video's share | vs the 16.74 ms budget |
|---|---|---|---|
| `rgba` 160×144 (default) | 0.223 | **~0.007** | 0.04 % |
| `rgb24` 160×144 | 0.227 | ~0.011 | 0.07 % |
| `yuyv` 640×480 | 0.770 | ~0.55 | 3.3 % |
| `yuyv` 640×480, `display` + LCD ×3 | 1.79 | ~1.58 | 9.4 % |

The frame rate rescued in [`docs/terminal-render.md`](terminal-render.md) is not disturbed: the
conversion is one pass over the frame with precomputed nearest-neighbour index tables, every
buffer is reused (no per-frame allocation), a duplicate frame re-writes the bytes it already
converted, and the whole thing sits on the emulation side of the loop where the budget is 16.7
ms, not on the encode side where it is contended.

Two honest caveats:

* **A slow consumer back-pressures the emulator.** Writes are blocking. `ffmpeg` keeps up at
  these rates with room to spare, but a consumer that stops reading a FIFO will stall the game
  (the audio queue notices first). A consumer that *exits* closes the pipe, and a failed write
  disables the stream permanently instead of erroring once per frame — the game keeps running.
* **Opening a FIFO blocks until a reader attaches.** That is standard FIFO behaviour and is why
  the stream is opened before the alternate screen, with a note on stderr — otherwise it looks
  exactly like a hang.

## 9. Platform

| | Frame stream | Virtual camera |
|---|---|---|
| **Linux** | yes | yes, via `v4l2loopback` + `ffmpeg` (§5) |
| **macOS** | yes (a pipe is a pipe) | not here. Use OBS's built-in virtual camera with a Media Source on the FIFO; a native path would be a CoreMedia IO camera extension. |
| **Windows** | yes | not here. OBS's virtual camera, or a DirectShow filter. |
| **wasm32** | not applicable — the module is `#[cfg(not(target_arch = "wasm32"))]`; the browser already has the canvas (and `captureStream()`). |

`v4l2` is Linux-only and this document does not pretend otherwise. The macOS and Windows
equivalents are named and deliberately left; both are reachable *today* by pointing OBS at the
same stream, which is the whole argument for producing frames rather than building a server.

## 10. Conformance record

Per the [standing convention](../CLAUDE.md#standing-convention-every-emulation-fix-feeds-the-conformance-tool).
This is an output feature, not an emulation fix, but two properties belong in the tool:

1. **The stream's rate is the emulator's, not the display's.**
   *Symptom* — a recording runs fast or slow, or loses frames, when the window is large or the
   emulator is fast-forwarded. *Caught by* — no test ROM (a framebuffer hash sees none of it);
   the telemetry invariant is `video.frames ≈ elapsed × 59.7275` with `speed_pct` at 100.
   *Contract* — `rawvideo` has no timestamps, so frame count *is* time.
   *New tests* — `realtime_pace_is_one_to_one_at_normal_speed`,
   `realtime_pace_drops_fast_forwarded_frames`,
   `a_paused_emulator_still_produces_a_steady_stream`,
   `the_video_stream_is_fed_by_emulated_frames_not_displayed_ones`.
2. **A frame-aligned stream, always.**
   *Symptom* — the picture tears diagonally or the consumer desynchronises partway through,
   typically after a mid-stream resolution change. *Caught by* — no test ROM.
   *Contract* — `-s WxH` is given once and every subsequent byte is positional.
   *New tests* — `the_stream_is_exactly_frames_times_frame_size`,
   `geometry_is_fixed_by_the_first_frame_and_survives_a_size_change`.

## 11. Screenshots

The still-image sibling, same frame source: **`c`** writes a PNG of what you are looking at
(palette + upscaler), **`C`** the raw 160×144 framebuffer, `F2` is an alias for `c`. No prompt,
no dialogue; a toast names the full path. Files land in `screenshot_dir`, defaulting to
`$XDG_PICTURES_DIR/gameboy` (else `~/Pictures/gameboy`), named
`DMG_ACID2-20260813-142530.png` — cartridge title plus a UTC timestamp, with a `-2`, `-3` …
suffix so two presses in the same second never overwrite. The upscale factor used is the one
**you selected**, not the transmission factor, which with `transmit_scale = auto` is
deliberately different (that is right for the wire and wrong for a file).

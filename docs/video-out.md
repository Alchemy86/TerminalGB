# Video out — the picture as a stream, and as a webcam

> The captain: *"can we confirm it's also possible for our terminal to PROVIDE A VIDEO STREAM
> OUT for things to plumb into and play, or even a webcam to PICK IT UP DIRECTLY!! like a
> camera feed!"*

**Yes to both, and the webcam needs no second process.** The frame stream is in the emulator
([`src/video.rs`](../src/video.rs)), and so is the camera: point `video_out` at a
`v4l2loopback` node, press `v`, and the game appears in Chrome, Meet, Discord or OBS. The
emulator negotiates the device's format itself (`VIDIOC_S_FMT`) and writes the frames. The
`ffmpeg` bridge in [`tools/virtual-camera.sh`](../tools/virtual-camera.sh) still works and is
still the route on macOS/Windows, but it is no longer the one you are meant to use on Linux.

**Every `video_*` setting is safe to put in `config.toml`.** A configured destination says
*where* frames go; it does not send them, and the emulator always starts with the stream **off**
— `v` is what goes live (`--video-start` / `video_autostart = true` if you want it live from
launch). No destination can stop the emulator from starting either: not a FIFO with nothing
reading it, not a camera that does not exist. See §5a.

This is the **pixels** half of the emit surface in [`docs/debugging.md`](debugging.md).
[`docs/telemetry.md`](telemetry.md) publishes the *numbers* of every frame as NDJSON; this
publishes the frame.

---

## 1. The one-line version

```bash
# record what a headless run draws, straight into an MP4
cargo run --release --example video_out -- --rom game.gb --frames 600 \
  | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p gb.mp4

# stream a game you are PLAYING in the terminal, through a FIFO — press `v` to go live
mkfifo /tmp/gb.raw
ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i /tmp/gb.raw -pix_fmt yuv420p gb.mp4 &
cargo run --release --example terminal -- game.gb --video-out /tmp/gb.raw

# a game you are playing, in a window, straight down a pipe. The desktop frontend
# has no `v` key, so this one says go: --video-start.
cargo run --release --example desktop -- game.gb --video-out - --video-start \
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
| destination | `--video-out` | `GB_VIDEO_OUT` | `video_out` | `-`/`stdout`, a file, a **FIFO**, a **`/dev/videoN` camera**, a directory, `off` |
| live at launch | `--video-start` | `GB_VIDEO_START` | `video_autostart` | `false` (**default** — press `v`), `true` |
| pixel format | `--video-format` | `GB_VIDEO_FORMAT` | `video_format` | `rgba` (default), `rgb24`, `yuyv` |
| frame size | `--video-size` | `GB_VIDEO_SIZE` | `video_size` | `WxH`, e.g. `640x480` |
| picture source | `--video-source` | `GB_VIDEO_SOURCE` | `video_source` | `native` (default), `display` |
| pacing | `--video-pace` | `GB_VIDEO_PACE` | `video_pace` | `realtime` (default), `emulated` |
| recordings dir | — | — | `video_dir` | default `$XDG_VIDEOS_DIR/gameboy`, else `~/Videos/gameboy` |
| camera to verify | — | — | `video_device` | a `/dev/videoN` loopback node (label check only) |

A `/dev/videoN` destination ignores `video_format` and `video_size`: a camera negotiates its own
(§5). Everything here is **safe to set** — see §5a.

In the terminal frontend, **`v` starts and stops the stream while the game runs** — see §6. It
starts **off** whatever `video_out` says; `video_autostart` is the separate opt-in for "and go
live at launch". The desktop frontend has no `v` key, so a destination configured there without
`--video-start` streams nothing and says so on stderr.

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

## 5. The virtual webcam — run the game, press `v`, done

### Setup: one command, once, and then never again

```toml
# ~/.config/gameboy/config.toml
video_out = "/dev/video10"
```

That is the whole emulator-side configuration. Size and pixel format are **detected, not
configured**: a V4L2 node is negotiated as YUYV at 640×480 (the size every consumer accepts),
whatever `video_format` / `video_size` say, and whatever the driver answers with is what gets
written. There is nothing to keep in sync between two programs any more, which is exactly the
class of mistake that used to send a 160×144 RGBA stream at a bridge expecting 640×480 YUYV.

The one remaining setup step is the kernel module, which needs root **once**:

```bash
sudo dnf install akmod-v4l2loopback v4l2loopback-utils     # Fedora
sudo apt install v4l2loopback-dkms v4l-utils               # Debian/Ubuntu
sudo pacman -S v4l2loopback-dkms v4l-utils                 # Arch

# a camera that survives a reboot
echo v4l2loopback | sudo tee /etc/modules-load.d/v4l2loopback.conf
echo 'options v4l2loopback devices=1 video_nr=10 card_label="Game Boy" exclusive_caps=1' \
  | sudo tee /etc/modprobe.d/v4l2loopback.conf
sudo modprobe -r v4l2loopback; sudo modprobe v4l2loopback
```

`exclusive_caps=1` is what makes Chrome, Firefox and Zoom accept the device (without it the node
advertises both capture and output and some consumers skip it). The emulator prints these exact
commands if the module is missing — it is the only thing left that it cannot do for you.

### How it works

A V4L2 output node is not a file. Writing raw bytes at one and getting no error proves
**nothing**: an un-negotiated write is accepted and produces nothing a consumer can read. The
emulator therefore does what `ffmpeg -f v4l2` does:

1. `VIDIOC_QUERYCAP` — confirm the node is a video *output*. A real webcam is a capture device
   and is refused by name (`/dev/video1 is "Full HD webcam" — a real camera, which can only be
   read from`), with the loopback nodes on the machine listed as the thing you probably meant.
2. `VIDIOC_S_FMT` with `V4L2_BUF_TYPE_VIDEO_OUTPUT` + `V4L2_PIX_FMT_YUYV` at 640×480. **What
   comes back is authoritative** — the driver may adjust the geometry, and `bytesperline` is
   the row stride, which is not necessarily `width * 2`. Frames are re-strided if it asks.
3. `write(2)` one `sizeimage` frame at a time, non-blocking.

The picture is scaled by the same integer-nearest path everything else uses and letterboxed, not
stretched (§4): 160×144 → 480×432 centred in 640×480 with black bars.

### No new dependency, and why

The maintained V4L2 crates (`v4l`, `v4l2-sys-mit`) are permissively licensed but link
**libv4l2**, a *system* library. This repo has been bitten by exactly that before — `cpal`
pulling in ALSA made a fresh clone fail to build until audio was feature-gated — and the point
of this work is that a new user has *fewer* setup steps, not another `-devel` package. So the
kernel UAPI is spoken directly through the `libc` dependency that is already here, the same way
[`src/shmframe.rs`](../src/shmframe.rs) speaks POSIX shared memory.

Hand-writing UAPI structs is the risk that buys, and it is bounded by making a mistake a
**compile error**: an `ioctl` request number encodes `sizeof` the struct it carries, so
`VIDIOC_QUERYCAP` and `VIDIOC_S_FMT` are computed from our own definitions and then
`const`-asserted against the values the kernel headers publish (`0x80685600` / `0xc0d05605`). A
layout mistake cannot reach a running program. Everything is behind
`#[cfg(all(target_os = "linux", feature = "libc"))]`; other targets keep the `ffmpeg` recipe.

### The bridge is still there

[`tools/virtual-camera.sh`](../tools/virtual-camera.sh) still works and is still the answer on a
box with no `v4l2loopback`, and on macOS (OBS's virtual camera) and Windows (DirectShow). It now
reads a **sidecar descriptor** the emulator writes beside a FIFO (`<fifo>.gbvideo`) so its
geometry, pixel format and device label come from the stream rather than from a second copy of
the same numbers.

### Naming the camera after the game — what is actually possible

"POKEMON BLUE" in a camera picker beats "Dummy video device (0x0000)", and the honest answer is
that **the emulator cannot set it.** This is settled, not assumed:

* the label is the `card_label` module parameter, fixed when the device is *created*;
* `/sys/class/video4linux/videoN/name` is `-r--r--r--` and root-owned — checked on a live
  device, not inferred;
* `modinfo v4l2loopback` lists no other writable route (`dev_debug`, `format`, `max_openers`
  are the only writable sysfs attributes, and none of them is the name);
* upstream `v4l2loopback-ctl` has `add`, `delete`, `set-fps`, `set-caps`, `set-timeout-image`
  — and no rename;
* `VIDIOC_S_FMT` negotiates a *format*, never an identity.

So changing it means deleting and re-adding the device as root, which drops every consumer
attached to it. **A per-launch `sudo` prompt is not something this emulator will ever do**, so
the supported answer is a static label chosen once at setup (`card_label="Game Boy"` above),
and a per-game one only if you want to run the setup command again:

```bash
sudo v4l2loopback-ctl delete /dev/video10
sudo v4l2loopback-ctl add -n "POKEMON BLUE" /dev/video10
```

What the emulator *can* do it does: set `video_device = "/dev/video10"` in `config.toml` and it
reads the device's label from sysfs, checks it against the running cartridge, and tells you the
two commands to fix it if they disagree:

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

## 5a. No destination can stop the emulator from starting

This is a promise, and it has a regression test behind it
(`a_configured_fifo_with_no_consumer_does_not_stop_the_emulator_starting`).

It did not used to be true. `open(2)` on a **FIFO** for writing blocks until a reader attaches,
so `video_out = "/tmp/gb.raw"` in `config.toml` — which this page told people to write — parked
the emulator in `open` during startup, *before the terminal UI existed*. The user saw one line
of text and a process that could only be killed. `wchan` read `wait_for_partner`.

Three rules now, for every destination that has something at the far end:

1. **The open never blocks.** A FIFO is opened `O_NONBLOCK` and `ENXIO` ("no reader yet")
   becomes the state `waiting`, not a wait. A camera's `open` + `VIDIOC_S_FMT` are bounded
   driver calls, and a device that is missing, busy or not an output is also `waiting`.
2. **Attach late, and survive the far end leaving.** The open is retried every 250 ms while the
   stream is armed, so *starting the emulator first and the consumer second works exactly as
   well as the other order*. A consumer that quits gives us `EPIPE`; the stream drops back to
   `waiting` and picks up the next one. `SIGPIPE` is set to `SIG_IGN` before the first byte, so
   a consumer quitting can never kill the process — the FFI build is a `cdylib` where Rust's
   own runtime has not done that for us.
3. **A slow or stalled consumer costs frames, never frame time.** Writes are non-blocking; a
   full pipe buffer drops the frame and counts it (`stalled`). Nothing about the consumer's
   speed can become emulation time — the same rule the audio player had to learn
   (`docs/audio.md` §4c). Because `rawvideo` is positional, a frame that was only partly
   written is *finished* before the next one starts, so the consumer sees whole frames or none.

All three are worth keeping *even though* nothing is opened at startup any more (§5b): they are
what makes pressing `v` inside the alternate screen safe, and what makes late attach fall out
for free. Non-blocking is the property; deferring the open is not a substitute for it.

## 5b. Configuring a destination is not deciding to stream

The two are separate decisions and the emulator used to conflate them: **naming a destination
started the stream**, at launch, before the user had pressed anything. The captain's Status row
read `Video ◌ wait gameboy-cam.…` the moment his terminal came up.

That is wrong on its own terms — "where do frames go" and "send frames" are different
questions, and only one of them belongs in a config file that applies to every launch — and it
is the same mistake that produced the startup hang in §5a: the config file was treated as an
instruction to open something.

So:

* **A configured destination means "this is where `v` will send it".** Nothing is opened,
  nothing is created (opening a plain file *truncates* it, so a launch that opened one would
  quietly destroy the last recording), and no camera is claimed.
* **`v` is what goes live. Launch state is off**, always.
* **`video_autostart = true` / `--video-start` / `GB_VIDEO_START=1`** is the separate, explicit
  way to ask for it live from launch. That is one extra word for the people who want it, rather
  than a surprise for everybody who does not.

Pinned by `a_configured_destination_does_not_mean_streaming`, over both a FIFO and a plain file,
which also asserts that nothing appears on disk.

The desktop frontend has no `v` key, so an armed-but-idle stream there would be unreachable; it
prints `video: <dest> configured but not streaming — add --video-start` and streams nothing.
This is why the piped one-liner in §1 carries `--video-start`.

## 6. Toggling it live (terminal frontend)

**`v` is the live feed. `V` is a recording. They never trade places.** It starts off (§5b), and it
survives being pressed repeatedly — which is not free, see below.

* **`v`** sends the game to `video_out` and stops sending it. With **no** live destination
  configured it says so (`set video_out (e.g. /dev/video10), or press V to record a file`) rather
  than quietly doing the other thing. That fallback is exactly how "stop the stream" once answered
  `REC stopped: 268 frames, 23.6 MB → /home/user/Videos/gameboy` at somebody who was thinking about
  his camera, and it is the same configure-versus-send confusion as §5b wearing a third hat.
* **`V` (shifted) always records a file**, whatever `video_out` says. Feeding a camera and keeping a
  recording are different jobs with different destinations, different formats and different endings
  — one is closed, the other is held — so they get different keys, and neither key ever performs the
  other's job on its behalf.
* The **Status panel**'s `Video` row distinguishes every state and says *where*:
  `off` · `◍ idle video10` (the game is not going out, the camera is still ours and is showing the
  idle card) · `◌ wait /tmp/gb.raw` (a FIFO armed with nothing on the other end yet) ·
  `● LIVE 12M video10` (feeding a consumer) · `● REC 12M POKEMON_BLUE-….raw` (writing a file).
  `LIVE` and `REC` are deliberately not interchangeable words: the row once said `LIVE` for a
  recording, which is how a camera that was never being fed looked like it was working.

### Stopping the feed does not stop the camera — it changes what the camera shows

`v` off used to *close* whatever was open, and that turned out to be two separate faults at once.

**It killed the camera.** The captain, mid-call: *"when we press `v` it doesn't just stop streaming
the screen, it KILLS the cam, so Google loses it if we WERE streaming."* A viewer had to go back to
the camera picker and choose the device again — unacceptable mid-call, and impossible mid-Twitch-stream.

**And it could not be undone.** Every distribution builds `v4l2loopback` with `exclusive_caps=1`, and
that flag means the node advertises `V4L2_CAP_VIDEO_OUTPUT` only while it has no consumer. The instant
the producer lets go of a device somebody is watching, `VIDIOC_QUERYCAP` on a fresh fd answers CAPTURE
and `VIDIOC_S_FMT` with `V4L2_BUF_TYPE_VIDEO_OUTPUT` answers `EINVAL`, for as long as the viewer stays
open. Measured against a real `ffmpeg` consumer on `/dev/video10`: **on/off/on worked once, and from
the second or third press the camera was gone and never came back.**

The principle that fixes both: **the device is a persistent output, and streaming the game into it is a
mode.** Exactly the same distinction as "where the video goes" versus "am I sending it" (§5b) — this is
that confusion one level further in. So the device is opened once and kept for the session, and `v`
switches only what is being *sent*. Never releasing it dissolves the re-acquire problem rather than
working around it: there is nothing left to re-acquire.

**Idle is not silence.** A third fault made that necessary rather than merely tidy: with the game feed
stopped, a consumer kept displaying the **last frame indefinitely**. The captain's Meet call sat on a
title screen while the emulator was in a Rattata battle — a dead feed that looks live, which on a public
stream is actively misleading. So while the game is off the destination keeps receiving an **idle card**
at [`IDLE_PERIOD`](../src/video.rs) (2 Hz):

![the idle card](idle-card.png)

* **Drawn in code, at the frame's own size.** No vendored asset, and the text is crisp at whatever the
  device negotiated rather than a 160×144 card magnified into blocks. It scales from 160×144 to 1280×720
  off one layout — sizes are derived from the canvas, not a table of magic numbers.
* **The font is this repository's own.** A 5×7 cell font written by hand for it, so it is MPL-2.0 like
  everything else here and there is no third-party licence question at all. A public-domain or OFL bitmap
  font would have been acceptable; a shaping library for ASCII glyph blitting would not.
* **It says nothing about the machine.** No game title, no path, no host. Switching the feed off is very
  often exactly the moment somebody does not want what they are doing on screen.
* **2 Hz, not 0 and not 60.** Consumers commonly drop a device that stops producing frames entirely, so
  idle has to be a trickle; the card is converted once and cached, so a tick is one `write(2)` of an
  unchanged buffer.

Recordings are the exception and are still **closed** on stop — that is what makes a recording a finished
file. The rule is [`Intent`](../src/video.rs): close a recording, hold anything live. A FIFO counts as
live, and deliberately so: its consumer is very often `tools/virtual-camera.sh`, whose `ffmpeg` owns the
camera, so an EOF there kills the device just as surely, one process further out.

Consequence worth knowing: only one destination is held at a time, so pressing `V` (record a file) while
a camera is idling *does* release the camera; the `?` overlay says so when it happens.

### A camera is never "waiting for a consumer"

A V4L2 output node accepts writes whether or not anybody is watching — that is the whole point of one —
so unlike a FIFO it is never blocked on a reader, and the sink never waits for one. But the *words* were
shared: any `Waiting` stream said `◌ waiting for a consumer on /dev/video10`, which sent the captain
hunting for a consumer problem when the real one was a second emulator instance still holding the device.
A camera that is not live now reports the device's own reason (`already being fed by another program`,
`v4l2loopback is not loaded`, `that is a real webcam`), because that is always what it is.

### Soak it yourself

```bash
cargo run --release --example video_toggle_soak -- /dev/video10 10
GB_SOAK_NO_PARK=1 cargo run --release --example video_toggle_soak -- /dev/video10 10
```

The second form restores the old always-release policy, which is how the soak is shown to have teeth.

**Measured, with `ffmpeg -f v4l2 -i /dev/video10` recording throughout and attaching *after* the emulator
was already running** — 1,028 consumer frames over 34.3 s, decoded frame by frame:

```
t=  0.00s    24 frames  GAME grey 40      <- consumer attached to a running emulator, picture appears
t=  0.80s    51 frames  IDLE CARD         <- `v` off: card, immediately, not a frozen game frame
t=  2.50s    57 frames  GAME grey 61      <- `v` on: game returns, same consumer session
...                                          (ten cycles, alternating, no gap and no re-pick)
t= 33.27s    30 frames  IDLE CARD
```

Every transition lands within one consumer frame (33 ms) of the toggle; the device was never lost, never
re-picked, `stalled=0`, `write_errors=0`, file descriptors flat at 5, and +108 frames / +68,812,800 bytes
per cycle with no drift. On exit `fuser /dev/video10` reports no holders and the node is back to `OUTPUT`,
free for the next producer. The old always-release policy under the same conditions: every cycle after the
first failed and the consumer received nothing but its initial frame.

The FIFO path was soaked the same way through a bridge-style reader that reopens the pipe: 10/10 cycles,
424 whole frames, every cycle's level in order.

## 7. What was actually captured (proof)

Run on this box, with the vendored MIT `dmg-acid2` ROM:

| Command | Result |
|---|---|
| `video_out --frames 300 \| ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p acid2.mp4` | 160×144, **300 frames, 23891/400 = 59.7275 fps**, 137 KB. Extracted frame 300 is the acid2 face, "HELLO WORLD!" and all. |
| `video_out --frames 240 --format yuyv --size 640x480 --source display --palette green --upscale lcd --factor 3 \| ffmpeg -f rawvideo -pix_fmt yuyv422 …` | 640×480, 240 frames, 59.7275 fps. Decoded frame: green palette, LCD grid, **pillar/letterboxed with black bars, not stretched**. |
| `video_out --frames 120 --pace realtime --out /tmp/gb.raw` with `ffmpeg` reading the FIFO | 120 frames, **duration 2.009 s** (120 / 59.7275 = 2.009). 0 duplicated, 0 dropped. |
| `video_out --out /dev/video10` | **Negotiated 640×480 YUYV and wrote frames.** See the camera row below. |
| `tools/virtual-camera.sh` | Found `/dev/video10` labelled "Game Boy", read the sidecar descriptor, bridged the FIFO. |

Artefacts land in `target/video-proof/` (gitignored). Reproduce all of it with the commands
above; nothing here needs a terminal, a window or a human.

### The camera, end to end — what was run and what was seen

A write that returns success is **not** evidence: an un-negotiated write to a V4L2 node is
accepted and produces nothing. Every claim below is a *consumer decoding the device*.

| Step | Consumer | What was observed |
|---|---|---|
| Terminal frontend with `video_out = "/dev/video10"`, no bridge anywhere | `ffmpeg -f v4l2 -i /dev/video10 → PNG` | The game, 640×480, right way up, correct shades, pillar/letterboxed. Status row `● LIVE … video10`, device `state` = `capture`. |
| Same, while `cpu_instrs` ran | same, 1 fps for 8 s | Frame 1 showed `10` in progress, frame 8 showed `10:ok 11` — **the picture moves**. |
| Same, Pokémon Blue intro | same, 6 fps for 2 s | 5 distinct frames of 12: the Gengar/Nidorino animation, mid-motion. |
| Same | **`ffplay -f v4l2 -i /dev/video10`**, window screenshotted | A real media player window displaying the game, live. |
| Consumer killed mid-stream | — | Emulator stayed `● LIVE`, `emu 60` (full speed), byte counter still climbing. |
| Second emulator started while the device was **busy** | — | Started normally, reported `already being fed by another program`, ran at full speed, streamed nothing. |
| …then the first emulator quit | `ffmpeg -f v4l2` | The waiting one **attached on its own** and the consumer saw *its* ROM. No restart. |
| `video_out --out /dev/video1` (a real UVC webcam) | — | Refused by name, listing `/dev/video10 ("Game Boy")` as the node meant. Emulator ran on. |
| `video_out --out /dev/video233` (no such node) | — | Started, `does not exist — retrying`, full speed, exit 0. |

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

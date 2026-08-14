#!/usr/bin/env bash
# Feed the emulator's video stream into a v4l2loopback device, so OBS, Zoom, a
# browser — anything that lists cameras — sees the Game Boy as a webcam.
#
#   tools/virtual-camera.sh [--device /dev/videoN] [--name "POKEMON BLUE"]
#                           [--size 640x480] [--format yuyv422] [--fifo /tmp/gb.raw]
#                           [--create] [--relabel] [--once] [--check]
#
# Start it in either order with the emulator: the FIFO's writer end is opened
# non-blocking, so the emulator waits for this bridge and this bridge waits for
# the emulator, and neither one hangs the other. By default it keeps the camera
# alive across the emulator stopping and starting (`--once` to exit instead).
#
# The emulator deliberately does NOT write to /dev/videoN itself: a V4L2 output
# device needs its format negotiated (VIDIOC_S_FMT) before the first write, and
# shipping ioctl code that no test can exercise (the kernel module needs root to
# install) would be worse than shipping nothing. ffmpeg does it properly, and
# throws in format conversion and scaling for free. See docs/video-out.md.
#
# Linux only. macOS has no v4l2 — use OBS's built-in virtual camera and point
# OBS at the same stream. Windows likewise (DirectShow).
set -euo pipefail

DEVICE=""
NAME=""
SIZE=""
FORMAT=""
FIFO="/tmp/gb.raw"
CHECK_ONLY=0
CREATE=0
RELABEL=0
ONCE=0
FPS="59.7275"
# Defaults, used only when the emulator has published nothing to go on.
DEFAULT_SIZE="640x480"
DEFAULT_FORMAT="yuyv422"

die() { echo "virtual-camera: $*" >&2; exit 1; }
say() { echo "virtual-camera: $*" >&2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --device)  DEVICE="${2:?--device needs a path}"; shift 2;;
    --name)    NAME="${2:?--name needs a value}"; shift 2;;
    --size)    SIZE="${2:?--size needs WxH}"; shift 2;;
    --format)  FORMAT="${2:?--format needs a pix_fmt}"; shift 2;;
    --fifo)    FIFO="${2:?--fifo needs a path}"; shift 2;;
    --create)  CREATE=1; shift;;
    --relabel) RELABEL=1; shift;;
    --once)    ONCE=1; shift;;
    --check)   CHECK_ONLY=1; shift;;
    -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
    *) die "unknown argument $1";;
  esac
done

[ "$(uname -s)" = "Linux" ] || die "v4l2loopback is Linux-only (this is $(uname -s)).
On macOS use OBS's built-in virtual camera and add a Media Source reading $FIFO;
on Windows, a DirectShow virtual camera. Both consume the same stream."

command -v ffmpeg >/dev/null || die "ffmpeg is not installed — it is what feeds the device."

# --- 0. what shape is the stream? -------------------------------------------
# `rawvideo` has no header: geometry and pixel format live only on the consumer's
# command line, so the same numbers get configured twice in two programs and the
# consumer decodes garbage rather than failing when they drift. That is exactly
# how a 160x144 rgba stream ended up pointed at a bridge expecting 640x480 yuyv.
# The emulator therefore publishes them beside the FIFO; read them rather than
# assume. Explicit flags still win. See `descriptor()` in src/video.rs.
DESCRIPTOR="$FIFO.gbvideo"
D_WIDTH=""; D_HEIGHT=""; D_FORMAT=""; D_FPS=""; D_TITLE=""
read_descriptor() {
  [ -r "$DESCRIPTOR" ] || return 1
  local key value
  while IFS='=' read -r key value; do
    case "$key" in
      width)  D_WIDTH="$value";;
      height) D_HEIGHT="$value";;
      format) D_FORMAT="$value";;
      fps)    D_FPS="$value";;
      title)  D_TITLE="$value";;
    esac
  done < "$DESCRIPTOR"
  [ -n "$D_WIDTH" ] && [ -n "$D_HEIGHT" ] && [ -n "$D_FORMAT" ]
}

if read_descriptor; then
  say "reading $DESCRIPTOR: ${D_WIDTH}x${D_HEIGHT} $D_FORMAT @ ${D_FPS:-$FPS}${D_TITLE:+ — $D_TITLE}"
  [ -n "$SIZE" ]   || SIZE="${D_WIDTH}x${D_HEIGHT}"
  [ -n "$FORMAT" ] || FORMAT="$D_FORMAT"
  [ -n "$NAME" ]   || NAME="$D_TITLE"
  [ -z "$D_FPS" ]  || FPS="$D_FPS"
else
  cat >&2 <<EOF
virtual-camera: no descriptor at $DESCRIPTOR yet. The emulator writes one when
it arms a FIFO stream, so this is normal the very first time. Assuming
${SIZE:-$DEFAULT_SIZE} ${FORMAT:-$DEFAULT_FORMAT} — if the emulator sends anything else the picture
will be garbage rather than an error, so either let it run once or pass
--size/--format matching its video_size/video_format.
EOF
fi
SIZE="${SIZE:-$DEFAULT_SIZE}"
FORMAT="${FORMAT:-$DEFAULT_FORMAT}"

# --- 1. is the kernel module there? ----------------------------------------
if [ ! -d /sys/module/v4l2loopback ]; then
  cat >&2 <<EOF
virtual-camera: the v4l2loopback kernel module is not loaded.

It needs root to install, once:
  Fedora:  sudo dnf install akmod-v4l2loopback v4l2loopback-utils
  Debian:  sudo apt install v4l2loopback-dkms v4l-utils
  Arch:    sudo pacman -S v4l2loopback-dkms v4l-utils

Then create the camera. The label is what OBS/Zoom/the browser SHOW in their
camera list, and it can only be chosen when the device is created — there is no
way to rename a live v4l2loopback device:
  sudo modprobe v4l2loopback video_nr=10 card_label="${NAME:-Game Boy}" exclusive_caps=1

Nothing here is destructive: without the module, use the plain pipe instead —
  ffmpeg -f rawvideo -pix_fmt $FORMAT -s $SIZE -framerate $FPS -i $FIFO out.mp4
EOF
  exit 2
fi

# --- 2. find a loopback device ---------------------------------------------
list_devices() {
  for d in /sys/class/video4linux/video*; do
    [ -e "$d" ] || continue
    printf '%s\t%s\n' "/dev/$(basename "$d")" "$(cat "$d/name" 2>/dev/null || echo '?')"
  done
}

# A loopback node is a *virtual* v4l2 device. Checking the driver symlink is not
# enough and was the bug that made this script report "no loopback device
# exists" on a box that had one: v4l2loopback nodes have no `device` symlink at
# all, so `device/driver` never resolves. They do resolve into
# /sys/devices/virtual/, which a real (USB/PCI) camera never does.
find_loopback() {
  for d in /sys/class/video4linux/video*; do
    [ -e "$d" ] || continue
    if [ "$(basename "$(readlink -f "$d/device/driver" 2>/dev/null || echo none)")" = "v4l2loopback" ] ||
       case "$(readlink -f "$d")" in */devices/virtual/*) true;; *) false;; esac; then
      printf '/dev/%s' "$(basename "$d")"
      return 0
    fi
  done
  return 1
}

[ -n "$DEVICE" ] || DEVICE="$(find_loopback || true)"

# --create: make one, named after the game. This is the only point at which the
# label can be chosen, which is why it lives here and not in the emulator.
if [ -z "$DEVICE" ] && [ "$CREATE" = "1" ]; then
  say "creating a loopback device labelled \"${NAME:-Game Boy}\" (needs root)"
  sudo v4l2loopback-ctl add -n "${NAME:-Game Boy}" /dev/video10 ||
    die "could not create /dev/video10"
  DEVICE="/dev/video10"
fi

[ -n "$DEVICE" ] || die "the module is loaded but no loopback device exists.
Create one named after the game — re-run with --create, or by hand:
  sudo v4l2loopback-ctl add -n \"${NAME:-Game Boy}\" /dev/video10
devices seen:
$(list_devices)"

label_of() { cat "/sys/class/video4linux/$(basename "$1")/name" 2>/dev/null || echo '?'; }
LABEL="$(label_of "$DEVICE")"
say "device $DEVICE, labelled \"$LABEL\""

# --- 3. does the label name the game? --------------------------------------
# v4l2loopback cannot be renamed at runtime (upstream README documents add /
# delete / set-fps / set-caps / set-timeout-image and no rename), so the only
# way to re-label is delete + add — which needs root and drops every consumer.
# It is therefore offered as --relabel rather than done behind the user's back.
squash() { printf '%s' "$1" | tr -dc '[:alnum:]' | tr '[:upper:]' '[:lower:]'; }
if [ -n "$NAME" ] &&
   [ "$(squash "$LABEL")" != "$(squash "$NAME")" ] &&
   ! printf '%s' "$(squash "$LABEL")" | grep -qF "$(squash "$NAME")"; then
  if [ "$RELABEL" = "1" ]; then
    say "re-labelling $DEVICE as \"$NAME\" (needs root; drops current viewers)"
    sudo v4l2loopback-ctl delete "$DEVICE" || die "could not delete $DEVICE"
    sudo v4l2loopback-ctl add -n "$NAME" "$DEVICE" || die "could not re-add $DEVICE"
    LABEL="$(label_of "$DEVICE")"
    say "device $DEVICE is now labelled \"$LABEL\""
  else
    cat >&2 <<EOF
virtual-camera: $DEVICE is labelled "$LABEL" but the game is "$NAME".
A v4l2loopback device cannot be renamed while it exists. Re-run with --relabel,
or do it by hand (needs root, and drops anything watching the camera):
  sudo v4l2loopback-ctl delete $DEVICE
  sudo v4l2loopback-ctl add -n "$NAME" $DEVICE
EOF
  fi
fi

[ "$CHECK_ONLY" = "1" ] && exit 0

# --- 4. run the bridge ------------------------------------------------------
[ -p "$FIFO" ] || { rm -f "$FIFO"; mkfifo "$FIFO"; }
cat >&2 <<EOF
virtual-camera: bridging $FIFO -> $DEVICE ($SIZE $FORMAT @ $FPS)
Start the emulator in either order — it waits for this bridge and this bridge
waits for it. In the TUI, set video_out in config.toml and press 'v':
  video_out    = "$FIFO"
  video_format = "$FORMAT"
  video_size   = "$SIZE"
or on the command line:
  cargo run --release --example terminal -- game.gb \\
      --video-out $FIFO --video-format $FORMAT --video-size $SIZE
EOF

# ffmpeg sees EOF the moment the emulator's last writer closes the FIFO — i.e.
# every time the user presses 'v' or quits the game. Exiting there would take the
# camera down with it and force a restart in a fixed order, which is the very
# thing this is meant to stop. So by default it re-opens and waits.
run_once() {
  ffmpeg -hide_banner -loglevel warning \
    -f rawvideo -pix_fmt "$FORMAT" -s "$SIZE" -framerate "$FPS" -i "$FIFO" \
    -f v4l2 -pix_fmt yuyv422 "$DEVICE"
}

if [ "$ONCE" = "1" ]; then
  exec ffmpeg -hide_banner -loglevel warning \
    -f rawvideo -pix_fmt "$FORMAT" -s "$SIZE" -framerate "$FPS" -i "$FIFO" \
    -f v4l2 -pix_fmt yuyv422 "$DEVICE"
fi

trap 'say "stopped"; exit 0' INT TERM
while :; do
  run_once || say "ffmpeg exited ($?) — waiting for the emulator again"
  sleep 0.5
done

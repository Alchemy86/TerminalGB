#!/usr/bin/env bash
# Feed the emulator's video stream into a v4l2loopback device, so OBS, Zoom, a
# browser — anything that lists cameras — sees the Game Boy as a webcam.
#
#   tools/virtual-camera.sh [--device /dev/videoN] [--name "POKEMON BLUE"]
#                           [--size 640x480] [--fifo /tmp/gb.raw] [--check]
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
SIZE="640x480"
FIFO="/tmp/gb.raw"
CHECK_ONLY=0
FPS="59.7275"

die() { echo "virtual-camera: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="${2:?--device needs a path}"; shift 2;;
    --name)   NAME="${2:?--name needs a value}"; shift 2;;
    --size)   SIZE="${2:?--size needs WxH}"; shift 2;;
    --fifo)   FIFO="${2:?--fifo needs a path}"; shift 2;;
    --check)  CHECK_ONLY=1; shift;;
    -h|--help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
    *) die "unknown argument $1";;
  esac
done

[ "$(uname -s)" = "Linux" ] || die "v4l2loopback is Linux-only (this is $(uname -s)).
On macOS use OBS's built-in virtual camera and add a Media Source reading $FIFO;
on Windows, a DirectShow virtual camera. Both consume the same stream."

command -v ffmpeg >/dev/null || die "ffmpeg is not installed — it is what feeds the device."

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
  ffmpeg -f rawvideo -pix_fmt yuyv422 -s $SIZE -framerate $FPS -i $FIFO out.mp4
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

if [ -z "$DEVICE" ]; then
  # A loopback node is one whose driver is v4l2loopback.
  for d in /sys/class/video4linux/video*; do
    [ -e "$d" ] || continue
    if [ "$(basename "$(readlink -f "$d/device/driver" 2>/dev/null || echo none)")" = "v4l2loopback" ]; then
      DEVICE="/dev/$(basename "$d")"
      break
    fi
  done
fi
[ -n "$DEVICE" ] || die "the module is loaded but no loopback device exists. Create one:
  sudo v4l2loopback-ctl add -n \"${NAME:-Game Boy}\" /dev/video10
devices seen:
$(list_devices)"

LABEL="$(cat "/sys/class/video4linux/$(basename "$DEVICE")/name" 2>/dev/null || echo '?')"
echo "virtual-camera: device $DEVICE, labelled \"$LABEL\"" >&2

# --- 3. does the label name the game? --------------------------------------
# v4l2loopback cannot be renamed at runtime (upstream README documents add /
# delete / set-fps / set-caps / set-timeout-image and no rename), so the honest
# move is to check and hand over the exact re-create command.
if [ -n "$NAME" ]; then
  squash() { printf '%s' "$1" | tr -dc '[:alnum:]' | tr '[:upper:]' '[:lower:]'; }
  if [ "$(squash "$LABEL")" != "$(squash "$NAME")" ] &&
     ! printf '%s' "$(squash "$LABEL")" | grep -qF "$(squash "$NAME")"; then
    cat >&2 <<EOF
virtual-camera: $DEVICE is labelled "$LABEL" but you asked for "$NAME".
A v4l2loopback device cannot be renamed while it exists. To re-label it (needs
root, and drops anything currently watching the camera):
  sudo v4l2loopback-ctl delete $DEVICE
  sudo v4l2loopback-ctl add -n "$NAME" $DEVICE
EOF
  fi
fi

[ "$CHECK_ONLY" = "1" ] && exit 0

# --- 4. run the bridge ------------------------------------------------------
[ -p "$FIFO" ] || { rm -f "$FIFO"; mkfifo "$FIFO"; }
cat >&2 <<EOF
virtual-camera: bridging $FIFO -> $DEVICE ($SIZE yuyv422 @ $FPS)
Now start the emulator writing to that FIFO, e.g.
  cargo run --release --example terminal -- game.gb \\
      --video-out $FIFO --video-format yuyv --video-size $SIZE
(or press 'v' in the TUI once video_out is set in config.toml)
EOF
exec ffmpeg -hide_banner -loglevel warning \
  -f rawvideo -pix_fmt yuyv422 -s "$SIZE" -framerate "$FPS" -i "$FIFO" \
  -f v4l2 -pix_fmt yuyv422 "$DEVICE"

#!/usr/bin/env bash
# Put a caption band under an `abfilm` cut and encode it.
#
#   tools/testrig/caption-film.sh <frames-dir> <out.mp4> <zoom> <line> [line...]
#
# `abfilm` composes the panes and captions each arm; what it cannot know is
# which prompt the entry state holds, because that lives in the pool's own
# `<NNNNN>.expect.json` beside the save state. So the band is written here, from
# that file, and the text is passed in rather than invented.
#
# The band is rendered ONCE and overlaid, instead of drawn per frame: a 1,000
# frame cut is 1,000 `magick` invocations otherwise, and `drawtext` would need
# the font path, which on this box contains `[wght]` — brackets `magick` reads
# as a frame specifier.
#
# **The panes are upscaled with nearest neighbour.** They are Game Boy pixels;
# any other filter invents shades the framebuffer does not contain, which is
# the categorical error `screen::halfblocks` is guarded against.
set -euo pipefail

frames=$1; out=$2; zoom=$3; shift 3
w=$(magick identify -format '%w' "$frames/000000.png")
h=$(magick identify -format '%h' "$frames/000000.png")
W=$((w * zoom)); H=$((h * zoom))
band=$(( 26 * ${#@} + 18 ))

cap=$(dirname "$out")/caption.png
args=(-size "${W}x${band}" xc:'#101216' -font Helvetica -pointsize 19 -fill '#e8ecf0')
i=0
for l in "$@"; do
    i=$((i + 1))
    args+=(-annotate "+14+$((i * 26 - 4))" "$l")
done
magick "${args[@]}" "$cap"

ffmpeg -y -hide_banner -loglevel error \
    -framerate 30 -i "$frames/%06d.png" -i "$cap" \
    -filter_complex "[0]scale=${W}:${H}:flags=neighbor,pad=${W}:$((H+band)):0:0:color=0x101216[v];[v][1]overlay=0:${H}" \
    -c:v libopenh264 -b:v 3M -pix_fmt yuv420p "$out"
echo "$out"

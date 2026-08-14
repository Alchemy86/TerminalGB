#!/bin/sh
# Turn a run's periodic screenshots into a watchable timelapse.
#
#     tools/claude_plays/timelapse.sh target/run9 [fps]
#
# `examples/claude_plays --shots <dir> --shot-every N` writes `frame-*.png`
# every N emulated frames; this stitches them at `fps` (default 12), which for
# the default N=300 is a 250x speed-up — a half-hour run becomes about seven
# seconds per hundred decisions.
#
# The result goes **in the run directory, under `target/`, and is never
# committed**: it is mostly game frames, and this repository's licence boundary
# allows screenshots of our own interface with a game incidental in them, not a
# gallery of the game itself. See `AGENTS.md`, "Licence boundary".
set -e

dir="${1:?usage: timelapse.sh <run-dir> [fps]}"
fps="${2:-12}"
out="$dir/timelapse.mp4"

count=$(find "$dir" -maxdepth 1 -name 'frame-*.png' | wc -l)
if [ "$count" -eq 0 ]; then
    echo "no frame-*.png in $dir — was the run given --shots?" >&2
    exit 1
fi

# H.264 under whichever encoder this ffmpeg was built with — a Fedora build has
# `libopenh264` and no `libx264`, and failing with "Encoder not found" is a poor
# way to lose a recording.
enc=""
for e in libx264 libopenh264; do
    if ffmpeg -hide_banner -encoders 2>/dev/null | grep -q " $e "; then
        enc="$e"
        break
    fi
done
if [ -z "$enc" ]; then
    echo "no H.264 encoder in this ffmpeg build" >&2
    exit 1
fi

# `-pattern_type glob` keeps the numeric order the filenames already encode.
# Nearest-neighbour scaling by 4, because a 160x144 video is unwatchable and
# anything smoother would blur pixel art.
ffmpeg -y -loglevel error \
    -framerate "$fps" -pattern_type glob -i "$dir/frame-*.png" \
    -vf 'scale=iw*4:ih*4:flags=neighbor' \
    -c:v "$enc" -pix_fmt yuv420p -b:v 1500k \
    "$out"

echo "$count frames -> $out"

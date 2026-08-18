#!/usr/bin/env bash
# Record a command running in a real terminal, as a PNG sequence and an mp4.
#
#   tools/testrig/record-terminal.sh <out-dir> <cols> <rows> -- <command...>
#
# There is no asciinema on this box, so this is the repository's own screenshot
# recipe (docs/agent/pewter-chain.md, docs/rendering.md) turned into a loop: a
# WezTerm window on **XWayland** — the only display server here that `import`
# can capture — driven with absolute paths, grabbed by window id at ~3 fps.
#
# Three things that are not obvious and each cost a run:
#   * a spawned terminal starts in $HOME, so every path on the `start --` line
#     must be absolute and --cwd is not optional;
#   * `pkill -f` matches this script's own command line, so the window is
#     closed by the pid captured at launch and never by pattern;
#   * the capture loop costs about half a core, so a wall-clock figure printed
#     by a recorded run is a recorded run's figure — say so rather than
#     quoting it as the tool's.
set -euo pipefail

out=$1; cols=$2; rows=$3; shift 3
[ "${1:-}" = "--" ] && shift
repo=$(cd "$(dirname "$0")/../.." && pwd)
fps=${REC_FPS:-3}
font=${REC_FONT:-10}
tail_s=${REC_TAIL:-6}          # seconds to hold the finished screen on camera

mkdir -p "$out/frames"
rm -f "$out"/frames/*.png

# The command is echoed with a `$` in front of it and then run — the caption is
# the command, verbatim, rather than a re-typing of it.
python3 -c 'import shlex,sys,os; print(" ".join(shlex.quote(a) for a in sys.argv[1:]).replace(os.path.expanduser("~"), "~"))' "$@" > "$out/command.txt"
cat > "$out/run.sh" <<EOF
cd "$repo"
printf '\033[1;36m\$\033[0m %s\n' "\$(cat '$repo/$out/command.txt')"
$(printf '%q ' "$@")
printf '\n\033[2m-- recording ends --\033[0m\n'
sleep $tail_s
EOF

env -u WAYLAND_DISPLAY DISPLAY=:0 wezterm-gui \
    --config enable_wayland=false \
    --config "initial_cols=$cols" --config "initial_rows=$rows" \
    --config "font_size=$font" --config enable_kitty_keyboard=false \
    start --cwd "$repo" -- /usr/bin/bash --noprofile --norc "$out/run.sh" \
    >"$out/wezterm.log" 2>&1 &
wez=$!

# Wait for the window, then grab it until the shell exits. The client list is
# read AFTER the terminal has answered, never before: a window asked for more
# cells than the screen holds is clamped, and the size before it answers is the
# size it was asked for.
wid=""
for _ in $(seq 1 60); do
    wid=$(DISPLAY=:0 xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*' | tail -1 || true)
    if [ -n "$wid" ]; then break; fi
    sleep 0.25
done
[ -n "$wid" ] || { echo "no window appeared" >&2; kill "$wez" 2>/dev/null || true; exit 1; }

i=0
t0=$(date +%s.%N)
while kill -0 "$wez" 2>/dev/null; do
    DISPLAY=:0 import -window "$wid" "$out/frames/$(printf '%05d' $i).png" 2>/dev/null || break
    i=$((i+1))
    sleep "$(python3 -c "print(max(0.0, 1.0/$fps - 0.28))")"
done
kill "$wez" 2>/dev/null || true
wait "$wez" 2>/dev/null || true

t1=$(date +%s.%N)
# **The mp4's frame rate is measured, never assumed.** `import` is most of the
# period and its cost moves with the window's size, so a hard-coded fps would
# play the recording faster or slower than it happened.
real=$(python3 -c "print(f'{$i/max(0.001,$t1-$t0):.3f}')")
printf 'frames %d\nelapsed_s %.2f\nfps %s\n' "$i" "$(python3 -c "print($t1-$t0)")" "$real" > "$out/timing.txt"
echo "captured $i frame(s) into $out/frames at $real fps"

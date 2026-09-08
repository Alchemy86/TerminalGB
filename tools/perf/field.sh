#!/bin/sh
# Run every row of `docs/measured/against-the-field.md` **interleaved**, on one
# core, in one session, and print the best round for each.
#
#   POKE_ROM=<rom> tools/perf/field.sh [rounds] [core]
#
# The doc's §2 already states this method — "the best of three runs interleaved
# with each other on the same core in the same minute" — and it was carried out
# by hand. That is the whole reason for this file: a cross-emulator comparison
# that is not interleaved measures the box's afternoon, and one that cannot be
# re-run is a number nobody can check. It calls `pyboy-bench.py` and
# `core_bench` rather than reimplementing either.
#
# ## It prints a matrix, and that is not presentation
#
# This emulator has **two independent mode axes** and a figure means nothing
# without both:
#
#   * the **picture engine** — `standard` (whole-scanline) or `identical`
#     (per-dot). What a frame looks like and what it costs to draw.
#   * the **timing core** — `accurate` (the default: peripherals stepped per
#     bus access) or `fast` (`SpeedFlags`, opt-in, `docs/modes.md`). What the
#     CPU's neighbours are allowed to skip.
#
# They are not the same question and a run has one of each. A single number
# quoted without both is the failure this table exists to prevent — see
# `docs/modes.md` and `docs/picture-modes.md`.
#
# `identical` **refuses every speed flag** (`Gameboy::set_speed_flags`), so its
# fast row is structurally absent rather than merely unmeasured. That hole is
# printed, because it is the guarantee that a fast-mode throughput figure can
# never be presented beside a per-dot conformance score.
#
# `core_bench` takes `--speed-mode`; it never reads `$GB_SPEED` (that is
# `SpeedFlags::from_env`, which only the frontends and the scenario runner
# call). Passing the env var to this bench sets nothing and reads as "the flag
# buys nothing", which is how a stale row gets published.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
BENCH=${GB_BENCH:-$REPO/target/release/examples/core_bench}
ROM=${POKE_ROM:-}
FRAMES=${GB_FRAMES:-7168}
[ -n "$ROM" ] || { echo "set POKE_ROM to a cartridge (none ships here)" >&2; exit 2; }
[ -x "$BENCH" ] || { echo "no core_bench at $BENCH — cargo build --release --example core_bench" >&2; exit 2; }
rounds=${1:-3}
core=${2:-4}

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
ours() { # name, ppu, extra args...
    n=$1 ppu=$2; shift 2
    GB_PPU="$ppu" taskset -c "$core" "$BENCH" --rom "$ROM" --frames "$FRAMES" \
        --no-pixels --repeat 1 --json "$@" >>"$tmp/$n"
}

echo "core $core  frames $FRAMES  rounds $rounds"
echo "load before: $(cut -d' ' -f1-3 /proc/loadavg)"
r=1
while [ "$r" -le "$rounds" ]; do
    if [ -z "${GB_NO_PYBOY:-}" ]; then
        taskset -c "$core" python3 "$REPO/tools/perf/pyboy-bench.py" "$ROM" "$FRAMES" 2>/dev/null \
            | while read -r line; do
                case $line in
                *batch:*) echo "$line" | sed 's/.*= \([0-9.]*\) ms\/frame.*/\1/' >>"$tmp/pyboy" ;;
                *loop:*)  echo "$line" | sed 's/.*= \([0-9.]*\) ms\/frame.*/\1/' >>"$tmp/pyboy_loop" ;;
                esac
            done
    fi
    ours standard_accurate  standard
    ours standard_fast      standard --speed-mode
    ours identical_accurate identical
    r=$((r + 1))
done
echo "load after:  $(cut -d' ' -f1-3 /proc/loadavg)"
echo

best() { [ -s "$1" ] && sort -g "$1" | head -1; }
pyms=$(best "$tmp/pyboy" || true)
pyloop=$(best "$tmp/pyboy_loop" || true)

printf '%-10s %-9s %10s %10s %10s   %s\n' picture timing ms/frame xrealtime 'vs pyboy' hashes
if [ -n "$pyms" ]; then
    printf '%-10s %-9s %10s %10s %10s\n' 'pyboy' '(no cyc)' "$pyms" \
        "$(awk "BEGIN{printf \"%.0f\", 1000/$pyms/59.7275}")" '-'
    [ -n "$pyloop" ] && printf '%-10s %-9s %10s %10s %10s   %s\n' '' 'py-loop' "$pyloop" \
        "$(awk "BEGIN{printf \"%.0f\", 1000/$pyloop/59.7275}")" '-' \
        '(its Python per-frame call — not the core; do not quote against us)'
fi
for n in standard_accurate standard_fast identical_accurate; do
    [ -s "$tmp/$n" ] || continue
    pic=${n%_*}; tim=${n##*_}
    ms=$(sed 's/.*"ms_per_frame":\([0-9.]*\).*/\1/' "$tmp/$n" | sort -g | head -1)
    xrt=$(awk "BEGIN{printf \"%.0f\", 1000/$ms/59.7275}")
    rel='-'
    [ -n "$pyms" ] && rel=$(awk "BEGIN{printf \"%.2fx\", $ms/$pyms}")
    h=$(sed 's/.*"frame_hash":"\([^"]*\)".*"wram_hash":"\([^"]*\)".*/\1 \2/' "$tmp/$n" | sort -u)
    tag=$(echo "$h" | head -1 | cut -c3-10)
    [ "$(echo "$h" | wc -l)" -gt 1 ] && tag=UNSTABLE
    printf '%-10s %-9s %10s %10s %10s   %s\n' "$pic" "$tim" "$ms" "$xrt" "$rel" "$tag"
done
printf '%-10s %-9s %10s %10s %10s   %s\n' identical fast '-' '-' '-' \
    'REFUSED by design: the per-dot engine takes no shortcut'
echo
echo "conformance belongs to the picture engine AND the timing core together;"
echo "quote a speed from this table only beside a score measured in the same cell."

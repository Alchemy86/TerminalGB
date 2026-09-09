#!/usr/bin/env bash
# How many concurrent Game Boys this box sustains, and what each one gets.
#
#   tools/perf/fanout.sh <rom.gb> [counts...]
#
# ## Why this exists rather than `core_bench --workers N` in a loop
#
# **A naive sweep measures the box's clock, not the emulator's scaling**, and
# it does it in the direction that manufactures a collapse. Measured here on an
# Intel Core Ultra 7 265: the P-cores hold 4,540 MHz for about five seconds of
# all-core load and then settle at **3,100 MHz** — a 32 % drop that stays for
# as long as the load does. With a fixed frame count per instance, a 20-worker
# point finishes in 0.7 s at burst clock and a 1000-worker point takes 51 s
# almost entirely at sustained clock, so the sweep reports a 40 % "scaling
# collapse" that is the silicon cooling policy and nothing to do with the fan
# out. The control is one line: the identical 20-worker 2,000-frame run reads
# **0.2145 ms/frame on a cold box, 0.3116 straight after a long one, and
# 0.2197 again after 45 s idle**, with the instance count never moving.
#
# So every point here is measured **at steady state**:
#
#   * `--warm-seconds` of real work before the timed window, at that point's
#     own load, so the clock has settled to what that many busy cores sustain;
#   * points run back to back with no idle, so the box never cools between
#     them;
#   * frames per instance scaled so every point's timed window is about the
#     same wall duration, which keeps all of them at the same place on the
#     curve;
#   * swept up and then down, both printed. **If the two disagree by more than
#     a few per cent the box was drifting and the run is not a measurement** —
#     the same rule `swarm.sh` states, and it is what caught this.
#
# Report the load average and what else is on the box, which is echoed below.
# An aggregate figure is wall clock by definition and cannot be corrected for
# somebody else's compile.
#
# **Set `GB_PPU`.** The default picture engine is `identical`; `standard` is
# about four times cheaper and every figure in `docs/measured/speed-ledger.md`
# was taken on it. This script does not set it — it prints what the binary
# reports having actually run.
set -uo pipefail

ROM=${1:?usage: fanout.sh <rom.gb> [counts...]}
shift
COUNTS=${*:-"1 20 50 100 200 500 1000"}
BIN=${BIN:-./target/release/examples/core_bench}
# Emulated frames in each point's timed window, before dividing by the instance
# count. Chosen so every point runs for roughly the same wall time.
BUDGET=${BUDGET:-400000}
WARM=${WARM:-6}
CFG=${CFG:---audio --no-pixels --no-audio-out}
# Refuse to start a point whose predicted footprint does not leave this much
# free. Per-instance cost is measured as we go, so the prediction is the
# machine's own, not a guess.
MARGIN_MIB=${MARGIN_MIB:-2048}

[ -x "$BIN" ] || { echo "no $BIN — cargo build --release --example core_bench" >&2; exit 2; }

field() { sed -n "s/.*\"$1\":\"\{0,1\}\([^,\"}]*\)\"\{0,1\}.*/\1/p"; }

point() { # $1 = instances -> one json line
  local n=$1 f
  f=$(( BUDGET / n )); [ "$f" -lt 500 ] && f=500; [ "$f" -gt 30000 ] && f=30000
  # shellcheck disable=SC2086
  "$BIN" --rom "$ROM" $CFG --frames "$f" --workers "$n" \
         --warm-seconds "$WARM" --repeat 1 --json 2>/dev/null
}

echo "rom     $ROM"
echo "cfg     $CFG   warm ${WARM}s   budget $BUDGET frames/point"
echo "cores   $(nproc)"
echo "load    $(uptime)"
echo "other   $(ps -eo pcpu,comm --sort=-pcpu | awk 'NR>1 && $1>20 {printf "%s(%s%%) ",$2,$1}')"
echo

per_inst_mib=0
declare -A UP DOWN
run_dir() { # $1 = direction, $2.. = counts
  local dir=$1; shift
  for n in $@; do
    local avail; avail=$(free -m | awk 'NR==2{print $7}')
    local want=$(( ${per_inst_mib%.*} * n + 64 ))
    if [ "$avail" -lt $(( want + MARGIN_MIB )) ]; then
      echo "  SKIP $n instances: predicts ${want} MiB, only ${avail} MiB free (margin ${MARGIN_MIB})" >&2
      eval "$dir[$n]=skip"
      continue
    fi
    local j; j=$(point "$n")
    local rss; rss=$(echo "$j" | field peak_rss_kib)
    [ -n "$rss" ] && per_inst_mib=$(awk -v r="$rss" -v n="$n" 'BEGIN{printf "%.3f", r/1024/n}')
    eval "$dir[$n]=\$j"
  done
}

# One full-load run first, so the very first point is not the only one measured
# on a cold box.
echo "warming the box to steady state..." >&2
"$BIN" --rom "$ROM" $CFG --frames 20000 --workers "$(nproc)" --repeat 1 --json >/dev/null 2>&1

run_dir UP $COUNTS
run_dir DOWN "$(echo "$COUNTS" | tr ' ' '\n' | tac | tr '\n' ' ')"

printf '%9s %11s %11s %12s %12s %10s %10s %9s\n' \
  copies ms/frame each_fps each_x_rt agg_fps agg_x_rt peakRSS_MiB ready_ms
for n in $COUNTS; do
  j=${UP[$n]}; k=${DOWN[$n]}
  [ "$j" = skip ] && { printf '%9s %11s\n' "$n" "not measured (memory)"; continue; }
  # Best of the two directions: a point that was unlucky with the box's other
  # work is measuring that, not this.
  a=$(echo "$j" | field aggregate_sps); b=$(echo "$k" | field aggregate_sps)
  best=$j; [ "${b:-0}" -gt "${a:-0}" ] 2>/dev/null && best=$k
  ms=$(echo "$best" | field ms_per_frame)
  agg=$(echo "$best" | field aggregate_sps)
  each=$(echo "$best" | field sps_per_instance)
  rss=$(echo "$best" | field peak_rss_kib)
  rdy=$(echo "$best" | field ready_ms)
  printf '%9s %11s %11s %12s %12s %10s %10s %9s\n' "$n" "$ms" \
    "$(awk -v x="$each" 'BEGIN{printf "%.0f",x}')" \
    "$(awk -v x="$each" 'BEGIN{printf "%.2f",x/59.7275}')" \
    "$agg" \
    "$(awk -v x="$agg" 'BEGIN{printf "%.0f",x/59.7275}')" \
    "$(awk -v x="$rss" 'BEGIN{printf "%.1f",x/1024}')" "$rdy"
  # Drift check, the reason both directions are run at all.
  awk -v a="${a:-0}" -v b="${b:-0}" -v n="$n" 'BEGIN{
    m=(a>b?a:b); l=(a<b?a:b); if (m>0 && (m-l)/m > 0.08)
      printf "  ! %s: up %d vs down %d — %.0f%% apart, the box drifted\n", n,a,b,100*(m-l)/m }'
done
echo
echo "picture $(echo "${UP[${COUNTS%% *}]}" | field picture)   timing $(echo "${UP[${COUNTS%% *}]}" | field timing)"
echo "load    $(uptime)"

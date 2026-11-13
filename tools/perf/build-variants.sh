#!/bin/sh
# Build `core_bench` under several release configurations, into separate target
# directories so nothing thrashes, and stash each binary under
# `target/perf/bin/<name>`.
#
# The point is an honest A/B: every variant is the same source, the same
# toolchain and the same example, differing only in the flags named here. The
# times printed are wall-clock build times from a **cold** target directory, so
# they answer "what does this cost to compile" as well as "what does it buy".
#
#   tools/perf/build-variants.sh [name ...]     # default: all of them
#
# See tools/perf/measure.sh for the run side and docs/throughput.md for what the
# numbers came out as.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$REPO"
OUT="$REPO/target/perf"
mkdir -p "$OUT/bin" "$OUT/td"
want="${*:-}"

build_one() {
    name=$1 opt=$2 lto=$3 cu=$4 flags=$5
    if [ -n "$want" ]; then
        case " $want " in
        *" $name "*) ;;
        *) return 0 ;;
        esac
    fi
    td="$OUT/td/$name"
    rm -rf "$td"
    printf '== %-16s opt=%-2s lto=%-4s cu=%-2s %s\n' "$name" "$opt" "$lto" "$cu" "$flags"
    start=$(date +%s.%N)
    CARGO_TARGET_DIR="$td" \
        CARGO_PROFILE_RELEASE_OPT_LEVEL="$opt" \
        CARGO_PROFILE_RELEASE_LTO="$lto" \
        CARGO_PROFILE_RELEASE_CODEGEN_UNITS="$cu" \
        RUSTFLAGS="$flags" \
        cargo build --release --example core_bench >/dev/null 2>&1
    end=$(date +%s.%N)
    cp "$td/release/examples/core_bench" "$OUT/bin/$name"
    secs=$(awk "BEGIN{printf \"%.1f\", $end - $start}")
    size=$(stat -c%s "$OUT/bin/$name")
    printf '   built in %ss, %s bytes\n' "$secs" "$size"
    echo "$name opt=$opt lto=$lto cu=$cu flags=${flags:--} build_s=$secs bytes=$size" \
        >>"$OUT/build-times.txt"
}

: >"$OUT/build-times.txt"
build_one baseline      s thin 1  ''
build_one o2            2 thin 1  ''
build_one o3            3 thin 1  ''
build_one o3-fat        3 fat  1  ''
build_one o3-fat-native 3 fat  1  '-Ctarget-cpu=native'
build_one s-fat-native  s fat  1  '-Ctarget-cpu=native'
build_one o3-fat-cu16   3 fat  16 ''

echo
echo "binaries in $OUT/bin"

#!/bin/sh
# What a build costs, in the three shapes that actually happen:
#
#   1. cold      — a clean target directory (a fresh clone, or CI)
#   2. core      — one line changed in `src/gameboy.rs`
#   3. frontend  — one line changed in `src/screen/tui.rs`
#
#   tools/perf/build-times.sh [profile ...]     # default: release quick dev
#
# Each profile gets its own target directory so they cannot warm each other, and
# the edit is a comment appended and then removed, so the source is unchanged
# afterwards. Report the load average with the numbers: this box runs other
# people's work and a build time taken under load 40 is not the same figure as
# one taken idle.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$REPO"
OUT="$REPO/target/perf"
mkdir -p "$OUT"
profiles=${*:-"release quick dev"}

CORE=src/gameboy.rs
FRONT=src/screen/tui.rs
MARK='// build-time probe'

# `sed`'s address delimiter is `/`, and the marker starts with `//`, so a naive
# `sed "/^$MARK$/d"` silently deletes the wrong thing and leaves the probe lines
# behind. Use a delimiter the marker cannot contain.
cleanup() {
    sed -i "\\%^$MARK\$%d" "$CORE" "$FRONT" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

timeit() { # timeit <cmd...>
    s=$(date +%s.%N)
    "$@" >/dev/null 2>&1 || { echo FAILED; return; }
    e=$(date +%s.%N)
    awk "BEGIN{printf \"%.1f\", $e - $s}"
}

echo "load: $(cut -d' ' -f1-3 /proc/loadavg)   cores: $(nproc)"
printf '%-10s %10s %10s %10s\n' profile cold core frontend
for p in $profiles; do
    td="$OUT/td/bt-$p"
    rm -rf "$td"
    case "$p" in
    dev) flag="" ;;
    release) flag="--release" ;;
    *) flag="--profile $p" ;;
    esac
    cleanup
    # shellcheck disable=SC2086
    cold=$(CARGO_TARGET_DIR="$td" timeit cargo build $flag --example terminal)
    echo "$MARK" >>"$CORE"
    # shellcheck disable=SC2086
    core=$(CARGO_TARGET_DIR="$td" timeit cargo build $flag --example terminal)
    cleanup
    echo "$MARK" >>"$FRONT"
    # shellcheck disable=SC2086
    front=$(CARGO_TARGET_DIR="$td" timeit cargo build $flag --example terminal)
    cleanup
    printf '%-10s %10s %10s %10s\n' "$p" "$cold" "$core" "$front"
done
echo "load: $(cut -d' ' -f1-3 /proc/loadavg)"

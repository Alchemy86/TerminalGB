#!/usr/bin/env bash
# Build the embedding core for other CPU architectures and RUN it, comparing
# the result against this machine's.
#
# The README and CLAUDE.md used to be able to say only that the core
# *type-checks* for other targets. A type check cannot tell you that a
# 32-bit machine has no 64-bit atomics, that a `usize` assumption leaked into
# the save-state format, or that a shift is architecture-dependent. Running it
# can. Every target below produces the same 160x144 framebuffer from the same
# cartridge, and this script fails if any of them disagrees with the host by so
# much as one pixel.
#
#   ./tools/run-cross.sh            # or: make test-cross
#
# What it needs, and what it does without:
#
#   * `rust-lld`, which every rustup toolchain already ships — so no C
#     cross-toolchain is installed, and none is wanted. That is why the targets
#     are all **musl**: rustup ships a prebuilt `libc.a` for those, and rust-lld
#     links them statically with nothing else on the machine.
#   * `qemu-<arch>-static`, to run a foreign binary. If it is missing the target
#     is SKIPPED, loudly, not silently passed. On Fedora that is
#     `qemu-user-static`; on Debian/Ubuntu `qemu-user-static`; without root,
#     `dnf download qemu-user-static-aarch64 && rpm2cpio *.rpm | cpio -idmu`
#     into a directory and point `$GB_QEMU_DIR` at its `usr/bin`.
#
# `$GB_QEMU_DIR` is searched before `$PATH`.
set -u

cd "$(dirname "$0")/.."
ROM=third_party/dmg-acid2/dmg-acid2.gb
FRAMES=${GB_CROSS_FRAMES:-300}

if [ ! -f "$ROM" ]; then
    echo "run-cross: $ROM is missing (it is vendored; check out the repo fully)" >&2
    exit 1
fi

# triple:qemu-binary  ("native" = this machine can execute it directly)
TARGETS="
aarch64-unknown-linux-musl:qemu-aarch64-static
armv7-unknown-linux-musleabihf:qemu-arm-static
i686-unknown-linux-musl:native
"
# Deliberately NOT in that list: `riscv64gc-unknown-linux-musl`. It is a rustup
# target and the emulator compiles for it, but the package declares
# `crate-type = ["cdylib", "rlib", "staticlib"]` (the FFI build), and a musl
# target that *supports* `cdylib` then tries to link one against a shared libc
# that rustup's musl sysroot does not ship: `unable to find library -lc`. The
# three above escape it only because rustc drops `cdylib` for them with a
# warning. That is a packaging quirk of this repository, not a portability
# finding about RISC-V — do not read it as one.

find_qemu() {
    if [ -n "${GB_QEMU_DIR:-}" ] && [ -x "$GB_QEMU_DIR/$1" ]; then
        printf '%s' "$GB_QEMU_DIR/$1"
        return 0
    fi
    # Distributions disagree about the `-static` suffix; accept either.
    command -v "$1" 2>/dev/null && return 0
    command -v "${1%-static}" 2>/dev/null && return 0
    return 1
}

# `frame hash` and `state bytes` are the two lines that must match: the first is
# every pixel, the second says the save-state layout did not change width with
# the pointer.
verdict() { grep -E '^(frame hash|state bytes)' | tr -s ' '; }

echo "== host $(uname -m): building the reference"
cargo build --release --no-default-features --example headless >/dev/null || exit 1
REFERENCE=$(./target/release/examples/headless "$ROM" "$FRAMES" | verdict)
if [ -z "$REFERENCE" ]; then
    echo "run-cross: the host build printed no verdict" >&2
    exit 1
fi
echo "$REFERENCE" | sed 's/^/   /'

fail=0
ran=0
for entry in $TARGETS; do
    triple=${entry%%:*}
    qemu=${entry#*:}
    echo
    echo "== $triple"

    if ! rustup target list --installed 2>/dev/null | grep -qx "$triple"; then
        if ! rustup target add "$triple" >/dev/null 2>&1; then
            echo "   SKIP: rustup has no std for $triple on this toolchain"
            continue
        fi
    fi

    runner=""
    if [ "$qemu" != "native" ]; then
        if ! runner=$(find_qemu "$qemu"); then
            echo "   SKIP: $qemu not found (install qemu-user-static, or set \$GB_QEMU_DIR)"
            continue
        fi
    fi

    # `rust-lld` rather than a C cross-compiler, so this needs nothing installed.
    var="CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER"
    if ! env "$var=rust-lld" \
        cargo build --release --no-default-features --example headless \
        --target "$triple" >/dev/null 2>"/tmp/gb-cross-$triple.log"; then
        echo "   FAIL: build"
        sed 's/^/      /' "/tmp/gb-cross-$triple.log" | tail -20
        fail=1
        continue
    fi

    bin="target/$triple/release/examples/headless"
    out=$($runner "$bin" "$ROM" "$FRAMES" 2>&1 | verdict)
    ran=$((ran + 1))
    if [ "$out" = "$REFERENCE" ]; then
        echo "   PASS: identical to the host"
        echo "$out" | sed 's/^/      /'
    else
        echo "   FAIL: differs from the host"
        echo "$out" | sed 's/^/      /'
        fail=1
    fi
done

echo
if [ "$fail" -ne 0 ]; then
    echo "run-cross: at least one target disagreed with the host"
    exit 1
fi
if [ "$ran" -eq 0 ]; then
    echo "run-cross: nothing was RUN (no qemu, no targets) — this proved nothing"
    exit 0
fi
echo "run-cross: $ran target(s) ran a cartridge and agreed with the host, pixel for pixel"

#!/usr/bin/env bash
# Build, flash and test the M5Stack PaperColor frontend.
#
# The toolchain is NOT the one the rest of this repository uses: upstream rustc
# has no Xtensa backend, so everything here goes through the `espup` fork. See
# platform/m5paper/README.md for the one-time install.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate="$here/platform/m5paper"
export_file="${ESPUP_EXPORT_FILE:-$HOME/.espup-export.sh}"
port="${M5PAPER_PORT:-/dev/ttyACM0}"
elf="$crate/target/xtensa-esp32s3-none-elf/release/terminalgb-m5paper"

# THE CARTRIDGE, compiled into the firmware's flash. No ROM is committed to this
# repository, so the default is the MIT dmg-acid2 image in third_party/ — the one
# cartridge this project may redistribute — and a fresh clone therefore builds
# and shows a picture with no game at all. Point it somewhere else to play one:
#
#   TERMINALGB_ROM=~/roms/blue.gb tools/m5paper-build.sh flash
#
# It is an absolute path because `include_bytes!` resolves relative paths
# against the source file, not the working directory.
export TERMINALGB_ROM="${TERMINALGB_ROM:-$here/third_party/dmg-acid2/dmg-acid2.gb}"
if [ ! -r "$TERMINALGB_ROM" ]; then
  echo "TERMINALGB_ROM is not readable: $TERMINALGB_ROM" >&2
  exit 1
fi
TERMINALGB_ROM="$(cd "$(dirname "$TERMINALGB_ROM")" && pwd)/$(basename "$TERMINALGB_ROM")"

usage() {
  cat <<'EOF'
usage: tools/m5paper-build.sh [command]

  build      compile the firmware (default)
  flash      compile, then flash over $M5PAPER_PORT (default /dev/ttyACM0)
  probe      flash the PANEL PROBE instead of the emulator, and watch it run
             (~7 min). It sweeps the PLL (0x30), the addressing window (0x83)
             and the DISPLAY_REFRESH parameter, timing every refresh, and ends
             by leaving a six-ink test card on the glass to be judged against
             `testcard`. Writes no waveform, voltage or LUT register.
             See src/bin/panel_probe.rs and docs/m5paper-refresh.md.
  monitor    attach to the serial log without resetting the board
  test       run the panel-geometry tests on the host (no device needed)
  testcard   render the expected picture to target/m5paper/ as PPM
  backup     read the whole 16 MB flash to a file before you overwrite it

  env: M5PAPER_PORT, ESPUP_EXPORT_FILE, TERMINALGB_ROM
EOF
}

need_toolchain() {
  if [ ! -f "$export_file" ]; then
    echo "no espup export file at $export_file" >&2
    echo "install it with:  espup install --targets esp32s3" >&2
    exit 1
  fi
  # shellcheck disable=SC1090
  . "$export_file"
}

# Serial access needs the dialout group. A shell that predates the user being
# added to it will not have it, and the only symptom is permission denied on the
# port, so re-enter the group explicitly rather than assuming.
run_on_device() {
  if [ -r "$port" ] && [ -w "$port" ]; then
    "$@"
  else
    sg dialout -c "$(printf '%q ' "$@")"
  fi
}

cmd="${1:-build}"
case "$cmd" in
  build)
    need_toolchain
    cd "$crate" && cargo build --release
    echo "built: $elf"
    ;;
  flash)
    need_toolchain
    cd "$crate" && cargo build --release
    echo "WARNING: this overwrites the factory firmware."
    echo "         run '$0 backup' first if you have not already."
    run_on_device espflash flash --chip esp32s3 --port "$port" --no-verify --non-interactive "$elf"
    ;;
  probe)
    need_toolchain
    probe_elf="$crate/target/xtensa-esp32s3-none-elf/release/panel_probe"
    cd "$crate" && cargo build --release --bin panel_probe
    echo "WARNING: this overwrites whatever firmware is on the device."
    run_on_device espflash flash --chip esp32s3 --port "$port" --no-verify \
      --non-interactive --monitor "$probe_elf"
    ;;
  monitor)
    run_on_device espflash monitor --port "$port" --non-interactive
    ;;
  test)
    # geom.rs is deliberately free of esp-hal, so it compiles for the host and
    # its rotation maths can be checked without a device in the room.
    out="$(mktemp -d)"
    rustc --test --edition 2021 -o "$out/geomtest" "$crate/src/geom.rs"
    "$out/geomtest"
    # gbscreen.rs is the same deal: no esp-hal, so the shade-to-ink mapping and
    # the picture's placement are checked rather than claimed. It pulls geom.rs
    # in itself under `cfg(test)`, so this runs both files' tests.
    rustc --test --edition 2021 -o "$out/gbscreentest" "$crate/src/gbscreen.rs"
    "$out/gbscreentest"
    rm -rf "$out"
    ;;
  testcard)
    mkdir -p "$here/target/m5paper"
    out="$(mktemp -d)"
    rustc --edition 2021 -O -o "$out/testcard" "$crate/src/testcard_host.rs"
    "$out/testcard" "$here/target/m5paper/panel-native.ppm" "${2:-landscape}"
    M5PAPER_VIEW=1 "$out/testcard" "$here/target/m5paper/as-seen.ppm" "${2:-landscape}"
    rm -rf "$out"
    echo "target/m5paper/panel-native.ppm  what sits in the framebuffer (portrait)"
    echo "target/m5paper/as-seen.ppm       what the eye sees (landscape)"
    ;;
  backup)
    dest="${2:-papercolor-factory-16mb.bin}"
    echo "reading 16 MB from $port to $dest (about 90 s)"
    run_on_device esptool --port "$port" --baud 921600 read-flash 0x0 0x1000000 "$dest"
    sha256sum "$dest" | tee "$dest.sha256"
    ;;
  -h|--help|help) usage ;;
  *) usage; exit 2 ;;
esac

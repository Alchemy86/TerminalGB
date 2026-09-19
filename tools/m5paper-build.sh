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
  pokedex    flash the SPECIES BROWSER instead of the emulator: all 151
             Pokemon front sprites, decompressed from your cartridge by
             `pixelgb species`, magnified and held on the glass one at a
             time. Needs the pictures:

               pixelgb species --rom ~/roms/blue.gb --out /tmp/species
               TERMINALGB_SPECIES_DIR=/tmp/species/pokemon-blue/sprites/four-shade/1x \
                 tools/m5paper-build.sh pokedex

             No game art is committed here, so there is no default and none
             can be invented. Button C advances to the next Pokédex entry, A
             goes back, B cycles the look. See src/bin/pokedex.rs and
             docs/m5paper-pokedex.md.
  charizard  an alias for `pokedex`, kept for the command this project used
             before it held more than one species. Accepts the old
             TERMINALGB_SPRITE_PNG (one file) in place of TERMINALGB_SPECIES_DIR
             (its directory) — the archive still holds everything pixelgb
             wrote next to it, so the browser opens with every species, not
             just the one the old variable named.
  probe      flash the PANEL PROBE instead of the emulator, and watch it run
             (~7 min). It sweeps the PLL (0x30), the addressing window (0x83)
             and the DISPLAY_REFRESH parameter, timing every refresh, and ends
             by leaving a six-ink test card on the glass to be judged against
             `testcard`. Writes no waveform, voltage or LUT register.
             See src/bin/panel_probe.rs and docs/m5paper-refresh.md.
  deepprobe  flash the DEEP probe: the experiments the probe above will not
             do. It writes registers nobody has documented for this controller
             (LUT_SEL, the PSR mode bits, a forced temperature) and cuts the
             power part-way through a waveform. It can leave the glass worse
             than it found it, and the maintainer authorised it on his own device.
             Resets and re-inits after every experiment and ends on a clean
             test card. See src/bin/panel_deep_probe.rs.
  truncate   flash the TRUNCATION stepper: it cuts the power part-way through
             a waveform and leaves the picture on the glass, stepped by the
             buttons. The one experiment here whose answer is a picture rather
             than a number, so somebody has to be holding the device.
             See src/bin/panel_truncate.rs.
  monitor    attach to the serial log without resetting the board
  test       run the panel-geometry tests on the host (no device needed)
  testcard   render the expected picture to target/m5paper/ as PPM
  backup     read the whole 16 MB flash to a file before you overwrite it

  env: M5PAPER_PORT, ESPUP_EXPORT_FILE, TERMINALGB_ROM, TERMINALGB_SPECIES_DIR
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
  pokedex|charizard)
    need_toolchain
    # THE PICTURES. Same discipline as TERMINALGB_ROM above and for the same
    # reason: a cartridge's pixels are not ours to ship, so every front sprite
    # is extracted from the player's own image by `pixelgb species` and named
    # from its own file. There is no default because there is nothing
    # redistributable to default to — refuse, and say what to run.
    #
    # `charizard` is the old command name, kept working: it took one file
    # (TERMINALGB_SPRITE_PNG) where this takes the directory beside it
    # (TERMINALGB_SPECIES_DIR) — `pixelgb species` with no `--species` filter
    # always writes every front sprite next to whichever one that variable
    # named, so the directory is right there and the browser opens with all
    # of them, not just the one the old variable pointed at.
    if [ -z "${TERMINALGB_SPECIES_DIR:-}" ] && [ -n "${TERMINALGB_SPRITE_PNG:-}" ]; then
      TERMINALGB_SPECIES_DIR="$(dirname "$TERMINALGB_SPRITE_PNG")"
    fi
    if [ -z "${TERMINALGB_SPECIES_DIR:-}" ]; then
      echo "TERMINALGB_SPECIES_DIR is not set: this build ships no game art." >&2
      echo >&2
      echo "  pixelgb species --rom /path/to/your.gb --out /tmp/species" >&2
      echo "  TERMINALGB_SPECIES_DIR=/tmp/species/pokemon-blue/sprites/four-shade/1x \\" >&2
      echo "    $0 pokedex" >&2
      exit 1
    fi
    if [ ! -d "$TERMINALGB_SPECIES_DIR" ]; then
      echo "TERMINALGB_SPECIES_DIR is not a directory: $TERMINALGB_SPECIES_DIR" >&2
      exit 1
    fi
    species_dir="$(cd "$TERMINALGB_SPECIES_DIR" && pwd)"

    # Build the archive on the host: `src/pokedex_host.rs` runs the same PNG
    # decode and shade quantiser `src/sprite_host.rs` uses for one file, once
    # per pokemon-front-*.png it finds, and writes them concatenated with a
    # directory in front — the format `src/pokedex.rs` reads. Neither
    # conversion is reimplemented here.
    mkdir -p "$here/target/m5paper"
    conv="$(mktemp -d)"
    rustc --edition 2021 -O -o "$conv/pokedex_host" "$crate/src/pokedex_host.rs"
    export TERMINALGB_POKEDEX="$here/target/m5paper/pokedex.bin"
    "$conv/pokedex_host" "$species_dir" "$TERMINALGB_POKEDEX"
    rm -rf "$conv"

    # `--features sprite` because the binary is gated: with no archive it
    # cannot compile at all, and a fresh clone must still build. See Cargo.toml.
    pd_elf="$crate/target/xtensa-esp32s3-none-elf/release/pokedex"
    cd "$crate" && cargo build --release --features sprite --bin pokedex
    echo "WARNING: this overwrites whatever firmware is on the device."
    run_on_device espflash flash --chip esp32s3 --port "$port" --no-verify \
      --non-interactive --monitor "$pd_elf"
    ;;
  probe)
    need_toolchain
    probe_elf="$crate/target/xtensa-esp32s3-none-elf/release/panel_probe"
    cd "$crate" && cargo build --release --bin panel_probe
    echo "WARNING: this overwrites whatever firmware is on the device."
    run_on_device espflash flash --chip esp32s3 --port "$port" --no-verify \
      --non-interactive --monitor "$probe_elf"
    ;;
  deepprobe)
    need_toolchain
    deep_elf="$crate/target/xtensa-esp32s3-none-elf/release/panel_deep_probe"
    cd "$crate" && cargo build --release --bin panel_deep_probe
    echo "WARNING: this writes undocumented registers to the panel."
    run_on_device espflash flash --chip esp32s3 --port "$port" --no-verify \
      --non-interactive --monitor "$deep_elf"
    ;;
  truncate)
    need_toolchain
    trunc_elf="$crate/target/xtensa-esp32s3-none-elf/release/panel_truncate"
    cd "$crate" && cargo build --release --bin panel_truncate
    run_on_device espflash flash --chip esp32s3 --port "$port" --no-verify \
      --non-interactive --monitor "$trunc_elf"
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
    # font.rs is pure (it plots through a closure) so it stands alone;
    # sprite.rs pulls gbscreen.rs and font.rs in itself, so this runs the
    # placement, the magnification and the ink mapping for the species still.
    rustc --test --edition 2021 -o "$out/fonttest" "$crate/src/font.rs"
    "$out/fonttest"
    rustc --test --edition 2021 -o "$out/spritetest" "$crate/src/sprite.rs"
    "$out/spritetest"
    # pokedex.rs pulls sprite.rs in the same way sprite.rs pulls gbscreen.rs —
    # this is the archive directory's own parser, checked without a device.
    rustc --test --edition 2021 -o "$out/pokedextest" "$crate/src/pokedex.rs"
    "$out/pokedextest"
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

#!/bin/sh
# Copy this into your emulator's repository (e.g. ci/gbconform-adapter.sh),
# change the two `exec` lines, and you are done.
#
#   jobs:
#     conformance:
#       uses: Alchemy86/gameboy/.github/workflows/conformance.yml@main
#       with:
#         build: cargo build --release
#         adapter: ./ci/gbconform-adapter.sh
#
# The full contract is in docs/conformance-action.md, or:
#   python3 tools/conformance/gbconform.py --contract
#
# What you get, per invocation:
#
#   $1 / $GBCONFORM_ROM   the ROM to run
#   $GBCONFORM_FRAMES     run at least this many frames, then stop
#   $GBCONFORM_MODEL      "dmg" or "cgb" -- USE IT. Some checks pin the console
#                         because the right answer differs by machine, so an
#                         adapter that hard-codes one reports a wrong verdict
#                         rather than a missing one.
#   $GBCONFORM_TIMEOUT_S  wall-clock budget before you are killed
#
# ...and at most one of these two is set, telling you what is wanted:
#
#   $GBCONFORM_SERIAL_OUT  write the raw bytes the ROM sends to SB here
#   $GBCONFORM_FRAME_OUT   write the final 160x144 frame here (PNG, Netpbm
#                          P6/P5/P3/P2, or a raw RGBA/RGB/8-bit dump)
#
# Exit 0 when the run completed. To DECLINE a capability you do not have,
# exit 0 having written nothing — the checks that need it are then reported
# skipped rather than failed. (Do not exit non-zero to decline: a non-zero
# exit means "I broke", and the runner deliberately treats that as a real
# failure of every check rather than as a missing feature.)
set -eu

EMU="${MY_EMULATOR:-./target/release/my-emulator}"

if [ -n "${GBCONFORM_SERIAL_OUT:-}" ]; then
    # Serial wanted. Tip: stop as soon as the ROM has printed Passed/Failed —
    # Blargg's ROMs loop forever afterwards, and running the full frame budget
    # every time turns a two-second suite into a two-minute one.
    exec "$EMU" --headless \
        --frames "$GBCONFORM_FRAMES" \
        --serial-out "$GBCONFORM_SERIAL_OUT" \
        "$GBCONFORM_ROM"
fi

if [ -n "${GBCONFORM_FRAME_OUT:-}" ]; then
    # A frame wanted. Run in the requested model: dmg-acid2 draws a different
    # (also correct) picture in CGB mode, and the reference image is the DMG
    # one. The same frame also carries the printed report of the ROMs that
    # never touch the link port, so getting the console right matters twice —
    # `oam_bug` passes on a DMG and fails on a Color console.
    exec "$EMU" --headless \
        --model "$GBCONFORM_MODEL" \
        --frames "$GBCONFORM_FRAMES" \
        --screenshot "$GBCONFORM_FRAME_OUT" \
        "$GBCONFORM_ROM"
fi

# Neither was asked for: nothing to do.
exit 0

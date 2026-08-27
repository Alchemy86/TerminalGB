#!/usr/bin/env bash
# Fetch mGBA, build its core, and build the blackout reference driver against it.
#
# mGBA is MPL-2.0 — the same licence as this repository — and is this project's
# approved reference emulator. Nothing is vendored: it is fetched, built and
# linked at run time, exactly as `tools/sameboy-oracle/` and `tools/bgb-interop/`
# do for theirs. Not in CI, for the same reason those are not.
#
#   tools/blackout/fetch-and-build.sh            # into target/mgba
#   MGBA_DIR=/somewhere tools/blackout/fetch-and-build.sh
#
# Then, from the repository root:
#
#   cargo run --release --features image,plugin-battle-control,plugin-trainer \
#       --example blackout_probe -- --rom <blue.gb>
#   target/mgba/mgba-blackout --rom <blue.gb> \
#       --party target/blackout-probe/party.bin \
#       --setup target/blackout-probe/setup.txt
set -euo pipefail

# The commit this was measured against. Bumping it is a deliberate act: mGBA's
# own behaviour is the reference, so a different commit is a different oracle —
# the same rule `tools/fetch-test-suites.sh` applies to every pinned suite.
MGBA_REF="${MGBA_REF:-c65e8a3d4666b0ea68a01578232452f31b185332}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="${MGBA_DIR:-$ROOT/target/mgba}"
SRC="$DIR/src"
BUILD="$DIR/build"

command -v cmake >/dev/null 2>&1 || {
  echo "error: no cmake on PATH"
  echo "help: python3 -m pip install --user cmake, then PATH=\$HOME/.local/bin:\$PATH"
  exit 2
}

mkdir -p "$DIR"
if [ ! -d "$SRC/.git" ]; then
  echo "fetching mGBA ($MGBA_REF) into $SRC"
  mkdir -p "$SRC"
  git -C "$SRC" init -q
  git -C "$SRC" remote add origin https://github.com/mgba-emu/mgba.git
  # A pin is a commit, not a branch, so it is fetched by sha rather than cloned.
  git -C "$SRC" fetch -q --depth 1 origin "$MGBA_REF"
  git -C "$SRC" checkout -q FETCH_HEAD
fi
echo "mgba at $(git -C "$SRC" rev-parse --short HEAD)"

if [ ! -f "$BUILD/libmgba.a" ]; then
  mkdir -p "$BUILD"
  # Core only: no Qt, no SDL, no GL. The driver needs the library and nothing
  # this box has to have installed.
  ( cd "$BUILD" && cmake "$SRC" \
      -DCMAKE_BUILD_TYPE=Release \
      -DBUILD_QT=OFF -DBUILD_SDL=OFF -DBUILD_GL=OFF -DBUILD_GLES2=OFF -DBUILD_GLES3=OFF \
      -DBUILD_SHARED=OFF -DBUILD_STATIC=ON -DBUILD_PYTHON=OFF -DBUILD_TEST=OFF \
      -DUSE_FFMPEG=OFF -DUSE_DISCORD_RPC=OFF -DUSE_LIBZIP=OFF -DUSE_MINIZIP=OFF \
      -DUSE_SQLITE3=OFF -DUSE_ELF=OFF -DUSE_EDITLINE=OFF -DUSE_LZMA=OFF \
      -DUSE_PNG=OFF -DUSE_ZLIB=OFF -DENABLE_SCRIPTING=OFF -DUSE_EPOXY=OFF \
      >/dev/null )
  ( cd "$BUILD" && make -j"$(nproc)" >/dev/null )
fi
echo "built $BUILD/libmgba.a"

# **Take the defines from mGBA's own build, and pass them as separate words.**
# Two traps, both of which produce a *segfault* rather than a compile error:
#
#  1. `mgba/flags.h` is generated for downstream users of an *installed* mGBA
#     and is NOT included by mGBA's own translation units. Force-including it
#     adds defines the library never saw, and `struct mCore` has fields behind
#     `#ifdef`s — so the two disagree about where `init` is.
#  2. `ENABLE_DIRECTORIES` is on the library's command line and is NOT in
#     `flags.h`, and it guards a by-value `struct mDirectorySet` member. Miss it
#     and every function pointer after it is at the wrong offset.
DEFS=()
while read -r d; do DEFS+=("$d"); done < <(
  grep -m1 "C_DEFINES" "$BUILD/CMakeFiles/mgba.dir/flags.make" | tr ' ' '\n' | grep '^-D'
)
[ "${#DEFS[@]}" -gt 0 ] || { echo "error: no defines found in mgba's flags.make"; exit 1; }

cc -O2 -I"$SRC/include" -I"$BUILD/include" "${DEFS[@]}" \
   -o "$DIR/mgba-blackout" "$ROOT/tools/blackout/mgba-blackout.c" \
   "$BUILD/libmgba.a" -lm -lpthread
echo "built $DIR/mgba-blackout"

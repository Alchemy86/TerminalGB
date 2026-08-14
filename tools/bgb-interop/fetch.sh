#!/bin/sh
# Fetch the counterparty for the interoperability check, at a pinned commit.
#
# Never vendored, for the reason `tools/fetch-test-suites.sh` gives for the test
# ROMs: it is somebody else's repository under somebody else's licence (MIT
# here), and a laptop, a CI job and a published claim should measure identical
# bytes. The pin lives here and nowhere else.
set -eu

COMMIT=0267d9a161f8275316c8497a1be04039dfa25131
REPO=https://github.com/Lorenzooone/PokemonGB_Online_Trades
DEST=${PGOT_DIR:-target/bgb-interop/PokemonGB_Online_Trades}

if [ -d "$DEST/.git" ]; then
    git -C "$DEST" fetch --quiet origin "$COMMIT" || true
else
    mkdir -p "$(dirname "$DEST")"
    git clone --quiet "$REPO" "$DEST"
fi
git -C "$DEST" checkout --quiet "$COMMIT"
echo "PokemonGB_Online_Trades at $COMMIT in $DEST"

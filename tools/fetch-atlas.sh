#!/bin/sh
# Vendor a pinned snapshot of an AtlasGB atlas, and check that the vendored
# copy is still the one that was pinned.
#
# This script is AtlasGB's own `tools/fetch-atlas.sh`, copied here as its
# docs/consuming.md asks consumers to do. Three adaptations, all noted where
# they appear: the destination defaults relative to *this script* rather than
# to the caller's working directory, ATLASGB_LOCAL can stand in for a published
# ref while the upstream repository has no release tag yet, and --file can name
# the path explicitly so a ref from before the layout moved is still pinnable.
# None of them changes the lock format, so upstream's copy and this one read
# each other.
#
#   tools/fetch-atlas.sh --ref <tag|branch|commit> [--atlas <id>]
#       Network. Fetch that ref, write the snapshot and a lock recording
#       atlas/ref/commit/sha256/date.
#
#   tools/fetch-atlas.sh --verify [--atlas <id>]
#       OFFLINE. Recompute the sha256 and compare it to the lock. This is the
#       check that matters day to day: it catches somebody hand-editing the
#       vendored copy instead of fixing it upstream. `testharness/gen1atlas.rs`
#       makes the same comparison in-process, so a plain `cargo test` catches
#       it too without shelling out.
#
#   tools/fetch-atlas.sh --check-upstream [--atlas <id>]
#       Network. Report whether upstream has moved past the pinned commit.
#       ADVISORY: exits 0 either way, because "upstream released something" is
#       news, not a build failure. `make atlas-upstream`.
#
# AtlasGB is an atlas *project*, not one atlas: `--atlas <id>` names which one,
# and Pokémon Red/Blue (`pokemon-rb`, the default, and the only one this
# repository has a use for) is its first. The vendored copy is namespaced the
# same way so a second atlas costs no restructuring here either.
#
# Why vendored rather than a submodule or a build-time download: a submodule
# breaks shallow CI checkouts and source tarballs and makes a one-file
# dependency cost a clone; a build-time download puts the network on the path
# of `cargo test`, and this atlas's structural checks are supposed to run on
# every plain test run. A committed snapshot shows up in the diff when it
# moves, which is the reviewable option. Same shape as `third_party/dmg-acid2/`,
# which this repository already vendors, plus the one thing that frozen artefact
# does not need — a lock, because this one is alive and will be re-released.
# `tools/fetch-gbselftest.sh` beside it vendors the GBSelfTest cartridge the
# same way; one pattern for every artefact consumed from outside this tree.
#
# Nothing but `curl` and `sha256sum` (or `shasum -a 256`), so vendoring the
# atlas adds no cargo dependency and `make embed`'s one-third-party-dependency
# rule is untouched.

set -eu

REPO="Alchemy86/AtlasGB"
NAME="atlas.tsv"
LOCK="atlas.lock"

# Adaptation 1: resolve the default destination from this script's own
# location, so the script works from any working directory.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

ATLAS="pokemon-rb"
REF=""
FILE=""
DEST=""
MODE="fetch"

usage() {
	sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//'
	exit "${1:-0}"
}

while [ $# -gt 0 ]; do
	case "$1" in
	--atlas) ATLAS="$2"; shift 2 ;;
	--ref) REF="$2"; shift 2 ;;
	--file) FILE="$2"; shift 2 ;;
	--dest) DEST="$2"; shift 2 ;;
	--verify) MODE="verify"; shift ;;
	--check-upstream) MODE="upstream"; shift ;;
	-h | --help) usage 0 ;;
	*) echo "unknown argument: $1" >&2; usage 2 ;;
	esac
done

: "${DEST:=$HERE/third_party/atlasgb/$ATLAS}"
: "${FILE:=atlases/$ATLAS/data/atlas.tsv}"

sha256() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	else
		shasum -a 256 "$1" | cut -d' ' -f1
	fi
}

lock_get() {
	# The lock is `key=value` per line: no JSON parser required, on purpose.
	sed -n "s/^$1=//p" "$DEST/$LOCK" 2>/dev/null || true
}

case "$MODE" in
verify)
	[ -f "$DEST/$NAME" ] || { echo "FAIL: $DEST/$NAME is missing" >&2; exit 1; }
	[ -f "$DEST/$LOCK" ] || { echo "FAIL: $DEST/$LOCK is missing" >&2; exit 1; }
	want="$(lock_get sha256)"
	got="$(sha256 "$DEST/$NAME")"
	if [ "$want" != "$got" ]; then
		echo "FAIL: $DEST/$NAME does not match $DEST/$LOCK" >&2
		echo "  locked $want" >&2
		echo "  actual $got" >&2
		echo "  The vendored copy is not the one that was pinned. Do not edit it" >&2
		echo "  here — fix it upstream at https://github.com/$REPO and re-pin." >&2
		exit 1
	fi
	echo "ok: $DEST/$NAME matches the lock ($(lock_get ref), ${got%"${got#??????????}"}…)"
	;;

upstream)
	pinned="$(lock_get commit)"
	head="$(curl -fsSL "https://api.github.com/repos/$REPO/commits/HEAD" 2>/dev/null |
		sed -n 's/^  "sha": "\([0-9a-f]*\)",*$/\1/p' | head -1)" || true
	if [ -z "$head" ]; then
		echo "note: could not reach github.com; nothing checked"
		exit 0
	fi
	if [ "$pinned" = "$head" ]; then
		echo "ok: pinned to the current head of $REPO ($pinned)"
	else
		echo "note: $REPO has moved on."
		echo "  pinned  $pinned  ($(lock_get ref), fetched $(lock_get fetched))"
		echo "  current $head"
		echo "  Re-pin with: $0 --atlas $ATLAS --ref <tag>"
		echo "  If the evidence tiers changed, that is a real update: they are"
		echo "  the reason to track this file rather than freeze it."
	fi
	;;

fetch)
	mkdir -p "$DEST"
	tmp="$(mktemp)"
	trap 'rm -f "$tmp"' EXIT
	# Adaptation 2: a local checkout can stand in for a published ref. AtlasGB
	# has cut no release tag yet, and this is also how a change to the data is
	# tried here before it is published upstream.
	if [ -n "${ATLASGB_LOCAL:-}" ]; then
		[ -f "$ATLASGB_LOCAL/$FILE" ] || {
			echo "no $ATLASGB_LOCAL/$FILE" >&2; exit 1; }
		cp "$ATLASGB_LOCAL/$FILE" "$tmp"
		REF="${REF:-$(cd "$ATLASGB_LOCAL" && git describe --tags --always --dirty 2>/dev/null || echo unreleased)}"
		commit="$(cd "$ATLASGB_LOCAL" && git rev-parse HEAD 2>/dev/null || echo unknown)"
		origin="local checkout $ATLASGB_LOCAL/$FILE"
	else
		[ -n "$REF" ] || { echo "give --ref (a tag, a branch or a commit), or set ATLASGB_LOCAL" >&2; exit 2; }
		url="https://raw.githubusercontent.com/$REPO/$REF/$FILE"
		echo "fetching $url"
		curl -fsSL "$url" -o "$tmp"
		# Resolve the ref to the commit it actually named, so the lock records
		# what was fetched rather than a moving label.
		commit="$(curl -fsSL "https://api.github.com/repos/$REPO/commits/$REF" |
			sed -n 's/^  "sha": "\([0-9a-f]*\)",*$/\1/p' | head -1)"
		[ -n "$commit" ] || commit="(unresolved)"
		origin="$url"
	fi
	head -1 "$tmp" | grep -q '^region	bank	addr' || {
		echo "FAIL: what came back is not an atlas (no header row)" >&2
		exit 1
	}
	mv "$tmp" "$DEST/$NAME"
	trap - EXIT
	cat >"$DEST/$LOCK" <<EOF
# Pinned AtlasGB snapshot. Written by tools/fetch-atlas.sh — do not hand-edit,
# and do not hand-edit $NAME either: fix it upstream and re-pin. The sha256
# below is what --verify, and testharness/gen1atlas.rs, compare against.
repo=https://github.com/$REPO
atlas=$ATLAS
file=$FILE
ref=$REF
commit=$commit
sha256=$(sha256 "$DEST/$NAME")
fetched=$(date -u +%Y-%m-%d)
origin=$origin
EOF
	echo "wrote $DEST/$NAME and $DEST/$LOCK"
	echo "  atlas  $ATLAS"
	echo "  ref    $REF"
	echo "  commit $commit"
	echo "  sha256 $(sha256 "$DEST/$NAME")"
	;;
esac

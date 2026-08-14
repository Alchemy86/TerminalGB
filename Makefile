.DEFAULT_GOAL := build

.PHONY: web desktop docs-links docs-check lint lint-audio test-audio-perf embed \
        fetch-test-suites test-external test-external-accurate

all: build install

watch:
	cargo watch -- make run

lint:
	cargo fmt -- --check --color always
	cargo clippy --all-targets -- -D warnings

# Audio is OFF by default (cpal needs ALSA dev headers, and a fresh clone must
# still build) — so `make lint` never compiles `sound.rs`'s cpal module at all.
# That is how four unused-variable warnings, each swallowing a real audio-device
# error, lived in the tree unnoticed. This target lints the feature; it is a
# SEPARATE target because it requires libasound2-dev / alsa-lib-devel, which
# `make lint` deliberately does not. CI runs both.
lint-audio:
	cargo clippy --all-targets --features audio -- -D warnings

# Code map (docs/codemap.md + docs/components/): rewrite every `path:line`
# reference into a fresh blob/main GitHub link and restamp the per-page
# "documented as of <sha>" marker. See docs/codemap.md §"How the links stay honest".
docs-links:
	python3 docs/tools/codemap_links.py

# Fail if any doc reference is broken: a missing file/dir target, or a line
# number past the end of its file. Suitable for CI.
docs-check:
	python3 docs/tools/codemap_links.py --check

build-wasm:
	cargo build --release --target wasm32-unknown-unknown
	wasm-opt -O4 ./target/wasm32-unknown-unknown/release/gameboy.wasm -o gameboy.wasm && du -h gameboy.wasm

test:
	cargo test --release

# The embedding build: the core with NO features. Proves the crate is usable
# from someone else's UI or hardware project — no window system, no terminal,
# no audio device, no image codecs, no $HOME. See README "Embedding the core".
# The `env -u` is the point: it fails if anything on this path needs a host.
embed:
	cargo clippy --release --no-default-features --all-targets -- -D warnings
	cargo test --release --no-default-features --lib
	cargo tree --no-default-features --edges normal
	env -u HOME -u XDG_CONFIG_HOME -u XDG_STATE_HOME -u DISPLAY -u WAYLAND_DISPLAY \
	  cargo run --release --no-default-features --example headless -- \
	  third_party/dmg-acid2/dmg-acid2.gb 240

# Emulation speed with a host audio player attached — the one thing no other
# benchmark measures, because they all run headless with no player. Needs the
# `audio` feature (so, ALSA dev headers) but NOT a sound card. See
# testharness/audioperf.rs and docs/audio.md.
test-audio-perf:
	cargo test --release --features audio --test audioperf

# The external suites (Mooneye GB, SameSuite, Mealybug Tearoom) are not
# vendored — they are other people's ROMs under other people's licences, so
# they are fetched at run time from the versions pinned in the script. See
# docs/testing.md, "Gating the external suites".
SUITES ?= target/test-suites

fetch-test-suites:
	./tools/fetch-test-suites.sh $(SUITES)

# Score every external suite and fail on ANY change from the checked-in
# expectations, in either direction — a test that starts passing is a baseline
# change too. Whole thing runs in about a second once the ROMs are on disk.
# Re-bless deliberately: `UPDATE_EXTERNAL_BASELINE=1 make test-external`.
test-external: fetch-test-suites
	cargo build --release --example mooneye_scout --example samesuite --example mealybug
	./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/acceptance $(SUITES)/mooneye/emulator-only \
	    --baseline testharness/mooneye_baseline.txt
	GB_MODEL=cgb ./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/misc \
	    --baseline testharness/mooneye_cgb_baseline.txt
	./target/release/examples/samesuite $(SUITES)/samesuite \
	    --baseline testharness/samesuite_baseline.txt
	./target/release/examples/mealybug --dir $(SUITES)/mealybug \
	    --baseline testharness/mealybug_baseline.txt

# The same suites again, against the OTHER PPU engine (src/gpu/fifo.rs). The
# baselines above are scored on whichever engine is the default, so the per-dot
# one — the whole point of which is these very suites — would otherwise be
# scored by nobody. Its own baselines carry its own pixel counts, so the next
# change to the pipeline shows up as a diff whether it moves a row or only moves
# the distance to one. Only the two PPU-bearing suites are re-scored; SameSuite
# is APU/DMA and does not distinguish the engines.
# Re-bless deliberately: `UPDATE_EXTERNAL_BASELINE=1 make test-external-accurate`.
test-external-accurate: fetch-test-suites
	cargo build --release --example mooneye_scout --example mealybug
	GB_PPU=accurate ./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/acceptance $(SUITES)/mooneye/emulator-only \
	    --baseline testharness/mooneye_accurate_baseline.txt
	GB_PPU=accurate ./target/release/examples/mealybug --dir $(SUITES)/mealybug \
	    --baseline testharness/mealybug_accurate_baseline.txt

desktop:
	cd examples/desktop && cargo run --release

desktop-dev:
	cd examples/desktop && cargo run

terminal-dev:
	cd examples/terminal && cargo run

terminal:
	cd examples/terminal && cargo run --release

web:
	cd web && npm run serve

web-build-dev:
	cd web && npm run build-dev

web-publish:
	cd web && npm run publish

web-build:
	cd web && npm run build

ffi-build:
# 	cargo install cbindgen
	cargo build --release --no-default-features --features ffi
	cbindgen . -o gameboy.h --lang c

ffi-size:
	du -h ./target/release/libgameboy.a

ffi:
	cp target/release/libgameboy.a examples/ffi-go/
	cd examples/ffi-go && go build main_static.go
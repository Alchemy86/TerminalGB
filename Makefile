.DEFAULT_GOAL := build

.PHONY: web desktop docs-links docs-check status lint lint-audio test-audio-perf embed \
        check-wasm fetch-test-suites test-external test-external-accurate no-plugins \
        test-mcp lint-mcp test-chiptune chiptune-demo bgb-interop

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

# Same reasoning for the MCP server: it is behind the `mcp` feature, so plain
# `make lint` never sees `src/mcp/` or `src/bin/terminalgb-mcp.rs`. No system
# library is needed for this one.
lint-mcp:
	cargo clippy --all-targets --features mcp -- -D warnings

# Code map (docs/codemap.md + docs/components/): rewrite every `path:line`
# reference into a fresh blob/main GitHub link and restamp the per-page
# "documented as of <sha>" marker. See docs/codemap.md §"How the links stay honest".
docs-links:
	python3 docs/tools/codemap_links.py

# Fail if any doc reference is broken: a missing file/dir target, or a line
# number past the end of its file. Suitable for CI. Also verifies the status
# page's suite scores still match the checked-in baselines (see `status`).
docs-check:
	python3 docs/tools/codemap_links.py --check
	python3 docs/tools/status_numbers.py --check

# Status page (docs/status.md + the Pages-ready docs/status/index.html):
# re-derive every suite score from testharness/*_baseline.txt and rewrite the
# pages. Run this in the SAME commit as any baseline re-bless — docs-check
# (in CI) is red until the pages agree with the baselines again.
status:
	python3 docs/tools/status_numbers.py

build-wasm:
	cargo build --release --target wasm32-unknown-unknown
	wasm-opt -O4 ./target/wasm32-unknown-unknown/release/terminalgb.wasm -o terminalgb.wasm && du -h terminalgb.wasm

test:
	cargo test --release

# Type-check both wasm builds: the embedding core and the web frontend
# (default features). This exists because GitHub CI only runs on push and this
# project merges into local main first — a wasm break on unpushed commits is
# invisible to CI for days, so the local gates must cover it. `rustup target
# add` is idempotent and a no-op once installed.
check-wasm:
	rustup target add wasm32-unknown-unknown
	cargo check --no-default-features --lib --target wasm32-unknown-unknown
	cargo check --target wasm32-unknown-unknown

# The embedding build: the core with NO features. Proves the crate is usable
# from someone else's UI or hardware project — no window system, no terminal,
# no audio device, no image codecs, no $HOME. See README "Embedding the core".
# The `env -u` is the point: it fails if anything on this path needs a host.
# Includes the wasm cross-check: `wasm32` compiles the `sound` module out, so
# it is the target most likely to break from a native-only change.
embed: check-wasm
	cargo clippy --release --no-default-features --all-targets -- -D warnings
	cargo test --release --no-default-features --lib
	cargo tree --no-default-features --edges normal   # blip_buf + first-party terminalgb-plugin-api
	env -u HOME -u XDG_CONFIG_HOME -u XDG_STATE_HOME -u DISPLAY -u WAYLAND_DISPLAY \
	  cargo run --release --no-default-features --example headless -- \
	  third_party/dmg-acid2/dmg-acid2.gb 240

# Emulation speed with a host audio player attached — the one thing no other
# benchmark measures, because they all run headless with no player. Needs the
# `audio` feature (so, ALSA dev headers) but NOT a sound card. See
# testharness/audioperf.rs and docs/audio.md.
test-audio-perf:
	cargo test --release --features audio --test audioperf

# The chiptune renderer, judged by properties of the audio it produces rather
# than by a hash of it: the envelope's 64 Hz counter, the sweep's overflow
# disable, the 7-bit LFSR's 127-bit period, the duty spectra, DMG-vs-CGB, the
# VGM round trip, and determinism. No ROM, no audio device, ~2 s.
# See testharness/chiptune.rs and docs/chiptune.md.
test-chiptune:
	cargo test --release --test chiptune

# Render the shipped demo phrase so a human can listen to it. Written for this
# tool — nothing sampled or borrowed. Play it with `paplay`, `aplay` or ffplay.
chiptune-demo:
	cargo run --release --bin terminalgb-chiptune -- \
	  docs/chiptune-examples/demo.gbscore -o target/chiptune-demo.wav
	@echo "listen with: paplay target/chiptune-demo.wav"

# The MCP server, driven the way a client drives it: the harness spawns the real
# `terminalgb-mcp` binary and speaks JSON-RPC down its stdin. Needs only the
# vendored dmg-acid2 ROM — no cartridge, no submodule, no network. See
# testharness/mcp.rs and docs/mcp.md.
test-mcp:
	cargo test --release --features mcp --test mcp

# Interoperability for the BGB link protocol: drive somebody else's
# implementation and check bytes cross. NOT in CI — it fetches another
# repository at run time and needs a Python interpreter, the same reason
# SameSuite is not. Run it by hand when src/bgblink.rs changes and record what
# it printed. See tools/bgb-interop/README.md and docs/bgb-link.md.
bgb-interop:
	./tools/bgb-interop/fetch.sh
	cargo build --release \
	    --features image,plugin-gen1-link,plugin-trainer,bgblink \
	    --example bgb_link
	python3 tools/bgb-interop/interop.py

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
	cargo build --release --example mooneye_scout --example samesuite \
	    --example mealybug --example cpp_suite
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
	./target/release/examples/cpp_suite $(SUITES)/cpp \
	    --baseline testharness/cpp_baseline.txt

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

# THE claim of the plugin system: the emulator is the product and the plugins
# are optional extras. Builds the whole thing — core, terminal and desktop
# frontends — with every plugin off, then greps the dependency graph to prove
# none of them is in it. A `cfg` that silently stopped compiling anything would
# pass the build and fail this. See docs/writing-a-plugin.md.
no-plugins:
	cargo build --no-default-features --features tui,desktop
	cargo test --no-default-features --lib
	@if cargo tree --no-default-features --features tui,desktop --edges normal \
	    | grep -E 'terminalgb-plugin-(trainer|gen1-link|battle-control|agent|example-tracer)'; \
	  then echo "FAIL: a plugin crate is in the no-plugin dependency graph"; exit 1; \
	  else echo "OK: no plugin crate in the dependency graph"; fi

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
	cbindgen . -o terminalgb.h --lang c

ffi-size:
	du -h ./target/release/libterminalgb.a

ffi:
	cp target/release/libterminalgb.a examples/ffi-go/
	cd examples/ffi-go && go build main_static.go
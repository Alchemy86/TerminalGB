.DEFAULT_GOAL := build

.PHONY: web desktop docs-links docs-check lint lint-audio

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
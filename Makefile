.DEFAULT_GOAL := build

.PHONY: test-gen1-atlas atlas-evidence atlas-upstream web desktop docs-links docs-check docs-index status picture-numbers controls-doc lint lint-audio test-audio-perf embed test-longplay \
	test-cross psp \
        check-wasm fetch-test-suites test-external test-external-accurate no-plugins \
        test-mcp lint-mcp test-agent-docs agent-docs-capture test-chiptune \
        chiptune-demo bgb-interop \
        oneclick play mcp agent doctor image image-smoke gbselftest

all: build install

watch:
	cargo watch -- make run

# `--features gen1` on purpose: `default` no longer carries the Generation 1
# work (see the root Cargo.toml's `gen1` feature), and lint coverage must not
# shrink just because the bundle moved out of the default set. The bare-default
# build is linted too, because "the emulator with no Pokemon code" is now a
# supported configuration and an unused field behind a plugin cfg is exactly
# the warning it catches.
lint:
	cargo fmt -- --check --color always
	cargo clippy --all-targets -- -D warnings
	cargo clippy --all-targets --features gen1 -- -D warnings

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
# page's suite scores still match the checked-in baselines (see `status`), that
# docs/picture.md's generated tables still match the source and the committed
# measurements (see `picture-numbers`), and that every page under docs/ is
# reachable from the index (see `docs-index`).
docs-check:
	python3 docs/tools/codemap_links.py --check
	python3 docs/tools/status_numbers.py --check
	python3 docs/tools/picture_numbers.py --check
	python3 docs/tools/docs_index.py --check

# docs/README.md is the documentation index. This checks every page under docs/
# is on it (or explicitly exempt with a reason), that every row it carries
# points at a page that exists and says what that page is for. Nothing is
# generated — the rule is coverage, because coverage is what drifted: when the
# gate was written, 57 of 101 pages were in no index at all and the best demo
# in the repository was one of them.
docs-index:
	python3 docs/tools/docs_index.py

# docs/picture.md: re-read the transport ladder, the upscaler table and the
# picture key bindings out of src/ and rewrite the generated blocks. The
# *measurements* it also quotes are data, not re-derived here — refresh those
# deliberately with `--measure`, which needs only the vendored dmg-acid2 ROM
# and a built `examples/render_bench`. docs-check (in CI) is red until the page
# agrees with both again.
picture-numbers:
	python3 docs/tools/picture_numbers.py

# docs/controls.md: regenerate the key table from src/screen/controls.rs, which
# is also what the in-app Controls panel renders from. A plain `cargo test`
# (in CI) is red until the page agrees with the registry again, so run this in
# the SAME commit as any key that moves. Prose outside the <!-- controls:… -->
# markers is preserved.
controls-doc:
	UPDATE_CONTROLS_DOC=1 cargo test --lib controls_doc

# Status page (docs/status.md + the Pages-ready docs/status/index.html):
# re-derive every suite score from testharness/*_baseline.txt and rewrite the
# pages. Run this in the SAME commit as any baseline re-bless — docs-check
# (in CI) is red until the pages agree with the baselines again.
status:
	python3 docs/tools/status_numbers.py

# The Gen 1 memory atlas. The DATA and every page rendered from it live at
# https://github.com/Alchemy86/AtlasGB, not here; a pinned snapshot is vendored
# at third_party/atlasgb/ and docs/gen1/atlas/README.md says why. What this
# repository owns is the EVIDENCE, because proving an address needs a running
# cartridge and upstream has neither one nor an emulator.
#
# The atlas harness. Its structural tier — the lock check, the structure, and
# the gate that every Gen 1 address hard-coded in a plugin is in the atlas —
# needs no cartridge and no network, and runs under a plain
# `cargo test --release`. Point POKE_ROM at your own Pokemon Red or Blue for the
# tiers that check the map against a running machine.
test-gen1-atlas:
	cargo test --release --test gen1atlas

# Produce a verification report and hand the evidence back to AtlasGB. Needs a
# cartridge. Land it in a clone of AtlasGB with `tools/apply-evidence.py`; the
# verify column is not editable by hand there, on purpose.
#
#   make atlas-evidence POKE_ROM=~/roms/pokeblue.gb
atlas-evidence:
	ATLAS_EVIDENCE_OUT=$(or $(ATLAS_EVIDENCE_OUT),target/atlas-evidence.json) \
	  cargo test --release --test gen1atlas

# Has AtlasGB released something newer than the snapshot we pinned? ADVISORY,
# and deliberately not part of any gate: a build must not break because somebody
# else published, and re-pinning moves what the harness scores, so it is a
# deliberate act in a commit of its own. Exits 0 either way, including with no
# network.
atlas-upstream:
	tools/fetch-atlas.sh --check-upstream

# `--profile wasm-release`, not `--release`: the shipping profile optimises for
# SPEED (opt-level 3, fat LTO) because every native consumer of it is CPU-bound,
# and this is the one target where bytes are the currency. See the two profiles'
# comments in Cargo.toml for both measurements.
build-wasm:
	cargo build --profile wasm-release --target wasm32-unknown-unknown
	wasm-opt -O4 ./target/wasm32-unknown-unknown/wasm-release/terminalgb.wasm -o terminalgb.wasm && du -h terminalgb.wasm

# `--features gen1` so the Generation 1 harnesses (trainer, scenario, agent,
# longplay, colosseum, portal, …) keep running: they moved out of `default`
# when the Pokemon bundle did, and a test suite that silently shrinks is worse
# than no change at all. `make no-plugins` is the other half — it proves the
# emulator still builds and passes its library tests with all of it off.
test:
	cargo test --release --features gen1

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

# Build the embedding core for other CPU architectures and RUN it, requiring a
# byte-identical frame and save-state length. Needs `qemu-<arch>-static` (a
# missing one SKIPS that target loudly, it does not pass it); the linker is
# rustup's own `rust-lld`, so no C cross-toolchain is installed.
# See docs/portability.md §2.
test-cross:
	./tools/run-cross.sh

# TerminalGB on a PlayStation Portable. Deliberately NOT part of `make test` or
# CI: it needs a pinned Rust nightly, a third-party std overlay and
# `-Z build-std`, and it produces an EBOOT nothing here can execute. Everything
# it fetches is pinned in the script. See docs/portability.md §3.
psp:
	./tools/psp-build.sh

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

# The agent documentation gate: every number, state name, public sentence and
# action verb printed in docs/agent-reference.md is checked against the code
# that produces it, and docs/agent-environment.md's frame hash and save-state
# size are recomputed from the vendored cartridge. Already covered by a plain
# `cargo test --release --features gen1` (which reaches plugin-agent); this
# target is for running it alone after editing either page.
# See testharness/agentdocs.rs.
test-agent-docs:
	cargo test --release --features plugin-agent --test agentdocs

# Regenerate every worked example under docs/agent/examples/ by driving the real
# binaries. Needs your own Pokemon Red/Blue cartridge — none is distributed here
# — and the three binaries named in the script's header.
agent-docs-capture:
	python3 tools/agent-docs/capture.py --rom $(ROM) --save $(SAV)

# THE LONG-RUN HARNESS: milestones out of a running machine, a durable log, a
# checkpoint another process resumes from, the episode primitive's reset loop,
# the per-decision dataset, the watch/perception separation, and the property
# the whole design rests on (a driver that never answers does not slow the
# emulator down). No ROM required, ~20 s. See docs/longplay.md.
test-longplay:
	cargo test --release --features longplay --lib longplay
	cargo test --release --features longplay --test longplay

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

# The external suites (Mooneye GB, SameSuite, Mealybug Tearoom, and the thirteen
# in c-sp/game-boy-test-roms) are not vendored — they are other people's ROMs
# under other people's licences, so they are fetched at run time from the
# versions pinned in the script. See docs/testing.md, "Gating the external
# suites".
SUITES ?= target/test-suites

# The suites in the c-sp aggregation this repository scores, each judged by its
# own author's convention. `examples/csp_suite.rs` is the one runner; the suite
# name selects the convention and the baseline. Keep this list sorted — it is
# also the order the scoreboard reads in.
CSP_SUITES = age bully cgb-acid-hell cgb-acid2 gambatte gbmicrotest \
             little-things-gb mbc3-tester mooneye-wilbertpol rtc3test \
             scribbltests strikethrough turtle-tests

fetch-test-suites:
	./tools/fetch-test-suites.sh $(SUITES)

# Score every external suite in the `standard` picture mode and fail on ANY
# change from the checked-in expectations, in either direction — a test that
# starts passing is a baseline change too. Whole thing runs in about a second
# once the ROMs are on disk.
#
# **`GB_PPU=standard` is spelled out on every line, and that is not decoration.**
# Until 2026-08-20 this target ran the default and the default was `standard`,
# so it could be silent about which engine it meant. `identical` is the default
# now (docs/picture-modes.md), so a target that named no engine would rewrite
# these eighteen files with the other machine's numbers the moment anybody
# re-blessed it. Both gates name their engine; neither file set changed hands.
# Re-bless deliberately: `UPDATE_EXTERNAL_BASELINE=1 make test-external`.
test-external: fetch-test-suites
	cargo build --release --example mooneye_scout --example samesuite \
	    --example mealybug --example cpp_suite --example csp_suite
	@for s in $(CSP_SUITES); do \
	    GB_PPU=standard ./target/release/examples/csp_suite --quiet --suite $$s \
	        --dir $(SUITES)/csp \
	        --baseline testharness/csp_$${s}_baseline.txt || exit 1; \
	done
	GB_PPU=standard ./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/acceptance $(SUITES)/mooneye/emulator-only \
	    --baseline testharness/mooneye_baseline.txt
	GB_PPU=standard GB_MODEL=cgb ./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/misc \
	    --baseline testharness/mooneye_cgb_baseline.txt
	GB_PPU=standard ./target/release/examples/samesuite $(SUITES)/samesuite \
	    --baseline testharness/samesuite_baseline.txt
	GB_PPU=standard ./target/release/examples/mealybug --dir $(SUITES)/mealybug \
	    --baseline testharness/mealybug_baseline.txt
	GB_PPU=standard ./target/release/examples/cpp_suite $(SUITES)/cpp \
	    --baseline testharness/cpp_baseline.txt

# GBSelfTest: the cartridge that tests the machine from the inside and prints a
# stable code per check (third_party/gbselftest/, docs/gbselftest.md). It is
# scored on BOTH picture modes and the gate is not "does it pass" -- the
# per-dot renderer must pass everything, and the whole-scanline renderer must
# lose exactly its five documented approximations and no more.
#
# It is kept out of tools/conformance/checks.json on purpose: that manifest is
# gated at 100% and its result is the README badge, and folding in five rows we
# fail by choice would force either a red gate or a lowered threshold.
gbselftest:
	./tools/fetch-gbselftest.sh --verify
	python3 docs/tools/gbselftest_doc.py --check
	cargo build --release --example conformance_adapter
	python3 tools/gbselftest-gate.py

# The same suites again in `identical`, the per-dot pixel FIFO in
# src/gpu/fifo.rs (docs/picture-modes.md) — **and since 2026-08-20 this is the
# gate on the DEFAULT engine**, which is why the three suites with only one
# baseline are scored here as well as above. `GB_PPU=accurate` is the pre-rename
# spelling of `identical`; it still parses, and leaving it here keeps the alias
# exercised in CI. The target keeps its name because it is quoted in
# docs/testing.md, docs/picture-modes.md, every baseline header and CI, and a
# rename buys nothing a comment cannot say.
#
# Its own baselines carry its own pixel counts, so the next change to the
# pipeline shows up as a diff whether it moves a row or only moves the distance
# to one. SameSuite, Mooneye `misc` and the CasualPokePlayer rows do not
# distinguish the modes — measured, all three are row-for-row identical under
# both — so they have one baseline each and it is scored twice rather than
# copied.
#
# The c-sp suites are re-scored here too, and they are where the two engines
# disagree most: the Gambatte suite alone moves by hundreds of rows and
# GBMicrotest by ~90, because both are largely mid-scanline PPU probes.
# Re-bless deliberately: `UPDATE_EXTERNAL_BASELINE=1 make test-external-accurate`.
test-external-accurate: fetch-test-suites
	cargo build --release --example mooneye_scout --example mealybug \
	    --example csp_suite --example samesuite --example cpp_suite
	GB_PPU=accurate ./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/acceptance $(SUITES)/mooneye/emulator-only \
	    --baseline testharness/mooneye_accurate_baseline.txt
	GB_PPU=accurate GB_MODEL=cgb ./target/release/examples/mooneye_scout \
	    $(SUITES)/mooneye/misc \
	    --baseline testharness/mooneye_cgb_baseline.txt
	GB_PPU=accurate ./target/release/examples/samesuite $(SUITES)/samesuite \
	    --baseline testharness/samesuite_baseline.txt
	GB_PPU=accurate ./target/release/examples/cpp_suite $(SUITES)/cpp \
	    --baseline testharness/cpp_baseline.txt
	GB_PPU=accurate ./target/release/examples/mealybug --dir $(SUITES)/mealybug \
	    --baseline testharness/mealybug_accurate_baseline.txt
	@for s in $(CSP_SUITES); do \
	    GB_PPU=accurate ./target/release/examples/csp_suite --quiet --suite $$s \
	        --dir $(SUITES)/csp \
	        --baseline testharness/csp_$${s}_accurate_baseline.txt || exit 1; \
	done

# THE claim of the plugin system: the emulator is the product and the plugins
# are optional extras. Builds the whole thing — core, terminal and desktop
# frontends — with every plugin off, then greps the dependency graph to prove
# none of them is in it. A `cfg` that silently stopped compiling anything would
# pass the build and fail this. See docs/writing-a-plugin.md.
no-plugins:
	cargo build --no-default-features --features tui,desktop
	cargo test --no-default-features --lib
	@if cargo tree --no-default-features --features tui,desktop --edges normal \
	    | grep -E 'terminalgb-plugin-(trainer|gen1-link|battle-control|agent|gen1-shinyhunt|gen1-lobby|example-tracer)|terminalgb-gen1-battle'; \
	  then echo "FAIL: a plugin crate is in the no-plugin dependency graph"; exit 1; \
	  else echo "OK: no plugin crate in the dependency graph"; fi

# ---------------------------------------------------------------------------
# One click: the container, the devcontainer, and the three ways to run it.
# See docs/quickstart.md. `tools/oneclick/terminalgb-run` is the same entry
# point the image uses as its ENTRYPOINT and a Codespace finds on its PATH, so
# these targets are shorthands rather than a fourth definition of anything.
# ---------------------------------------------------------------------------

# The build the container and the Codespace do: the terminal frontend, the MCP
# server and the agent runner. Deliberately NOT `default` — a container has no
# window (`desktop`) and no sound card (`audio`, which would pull ALSA in).
ONECLICK_FEATURES ?= tui,mcp,image

oneclick:
	cargo build --release --locked \
	    --no-default-features --features $(ONECLICK_FEATURES) \
	    --bin terminalgb --bin terminalgb-mcp --example claude_plays

play: oneclick
	./tools/oneclick/terminalgb-run play $(ROM)

mcp: oneclick
	./tools/oneclick/terminalgb-run mcp $(ROM)

agent: oneclick
	./tools/oneclick/terminalgb-run agent $(ROM)

doctor:
	./tools/oneclick/terminalgb-run doctor

# The shipped image. `--target dev` is what .devcontainer builds; `runtime` is
# what a user runs. Both are gated by .github/workflows/container.yml.
image:
	docker build --target runtime -t terminalgb:oneclick .
	@docker image ls terminalgb:oneclick

# The claim the image has to earn: it does not merely start, it emulates. Runs
# 120 frames through the MCP server and requires a 160x144 PNG back.
image-smoke: image
	@printf '%s\n' \
	  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"make","version":"0"}}}' \
	  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
	  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"step","arguments":{"frames":120}}}' \
	  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"screenshot","arguments":{}}}' \
	  > target/mcp-smoke-in.jsonl
	docker run --rm -i terminalgb:oneclick mcp < target/mcp-smoke-in.jsonl \
	  > target/mcp-smoke-out.jsonl
	python3 tools/oneclick/check_frame.py target/mcp-smoke-out.jsonl

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
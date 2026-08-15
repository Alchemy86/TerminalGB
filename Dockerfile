# syntax=docker/dockerfile:1
#
# TerminalGB in a container — the same definition the Codespace opens.
#
# Three stages, and the split is the point:
#
#   toolchain  the pinned Rust and the handful of system packages
#   dev        + the developer conveniences; THIS is what .devcontainer builds
#   runtime    a slim image with the three binaries and nothing else
#
# `.devcontainer/devcontainer.json` builds `--target dev` from this file, so a
# Codespace and `docker build` are the same environment rather than two
# definitions that drift apart. See docs/quickstart.md.
#
# What is pinned, and why (docs/quickstart.md §"What is pinned"):
#   * Rust 1.83.0        — the repository's own `rust-toolchain`; the crate sets
#                          rust-version = 1.83 and clippy is run with -D warnings,
#                          so a newer toolchain can fail the build on new lints.
#   * debian:bookworm    — glibc and Python major version stay put.
#   * cargo --locked     — Cargo.lock is tracked, so the dependency graph is the
#                          one CI measured. A build that would change it fails.
#
# What is deliberately NOT installed: any audio library. The `audio` feature is
# off, so this image needs no ALSA, no PulseAudio and no sound card — the APU is
# still fully emulated, there is just nowhere for it to play. Nor a display
# server: the `desktop` frontend is left out, because a container has no window.

ARG RUST_VERSION=1.83.0
ARG DEBIAN_SUITE=bookworm

# --------------------------------------------------------------------------
# toolchain — shared base
# --------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-${DEBIAN_SUITE} AS toolchain

ENV CARGO_TERM_COLOR=always \
    DEBIAN_FRONTEND=noninteractive

# ca-certificates: crates.io and the Anthropic API over TLS.
# git:             `cargo` fetches, and the optional Blargg submodule.
# python3:         the agent adapters in tools/claude_plays are stdlib-only
#                  Python. Nothing else in the project needs an interpreter at
#                  run time.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      git \
      python3 \
 && rm -rf /var/lib/apt/lists/*

# `rust-toolchain` asks for clippy, rustfmt and the wasm32 target on top of the
# base toolchain, so the first `cargo` call in any build would re-sync the whole
# channel. Doing it here bakes it into a cached layer instead: ~20 s off every
# build, and off Codespace creation, for one layer that never changes.
RUN rustup component add clippy rustfmt \
 && rustup target add wasm32-unknown-unknown

# --------------------------------------------------------------------------
# builder — compile the three things a user actually runs
# --------------------------------------------------------------------------
FROM toolchain AS builder

WORKDIR /src
COPY . .

# The feature set is chosen for a container, not copied from `default`:
#
#   tui    the terminal frontend — the one frontend that works over `docker
#          run -it` and over a Codespace's terminal
#   mcp    the Model Context Protocol server (pulls in plugin-agent and
#          chiptune; adds no third-party crate)
#   image  the PNG encoder the agent's screenshot channel needs
#
# Left out on purpose: `desktop` (no window system in a container), `audio` (no
# sound card, and it would drag ALSA in), `music`, `portal`, `bgblink`.
ARG FEATURES="tui,mcp,image"

# The cache mounts make a rebuild fast for a developer. They are anonymous
# volumes, so the artefacts have to be copied out inside this same RUN.
# `--no-cache` on the build line gives the honest cold-start number.
# The cache mounts make a rebuild fast for a developer. They are anonymous
# volumes, so the artefacts have to be copied out inside this same RUN. The
# explicit `id=`s keep them out of other projects' caches on a shared machine.
#
# `set -eux` rather than a `&&` chain: the chain would have needed a trailing
# `|| true` for the optional `strip`, and `A && B || true` swallows a failure in
# A as well — a cargo error would have produced an image with no binaries in it.
RUN --mount=type=cache,id=terminalgb-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=terminalgb-cargo-target,target=/src/target,sharing=locked \
    set -eux; \
    cargo build --release --locked \
        --no-default-features --features "${FEATURES}" \
        --bin terminalgb \
        --bin terminalgb-mcp \
        --example claude_plays; \
    mkdir -p /out; \
    cp target/release/terminalgb \
       target/release/terminalgb-mcp \
       target/release/examples/claude_plays \
       /out/; \
    strip /out/* || echo "strip unavailable — shipping unstripped binaries"

# --------------------------------------------------------------------------
# dev — what the Codespace opens
# --------------------------------------------------------------------------
FROM toolchain AS dev

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      make \
      curl \
      less \
      sudo \
      procps \
      jq \
 && rm -rf /var/lib/apt/lists/*

# A non-root user, the devcontainer convention. Codespaces will run as this
# user, and `sudo` is available for the apt-get a curious user will want
# (`libasound2-dev`, if they decide they want sound).
ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=1000
RUN groupadd --gid ${USER_GID} ${USERNAME} \
 && useradd --uid ${USER_UID} --gid ${USER_GID} -m -s /bin/bash ${USERNAME} \
 && echo "${USERNAME} ALL=(root) NOPASSWD:ALL" > /etc/sudoers.d/${USERNAME} \
 && chmod 0440 /etc/sudoers.d/${USERNAME}

# The rust image keeps CARGO_HOME and RUSTUP_HOME under /usr/local, which root
# owns; hand them to the user so `cargo build` does not need sudo.
RUN chown -R ${USER_UID}:${USER_GID} ${CARGO_HOME} ${RUSTUP_HOME}

USER ${USERNAME}
# The devcontainer mounts the clone under /workspaces/<folder> and sets its own
# working directory, so this is only a sensible landing spot for `docker run`.
WORKDIR /workspaces

# --------------------------------------------------------------------------
# runtime — the shipped image
# --------------------------------------------------------------------------
FROM debian:${DEBIAN_SUITE}-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      python3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 1000 -s /bin/bash gb

COPY --from=builder /out/terminalgb        /usr/local/bin/terminalgb
COPY --from=builder /out/terminalgb-mcp    /usr/local/bin/terminalgb-mcp
COPY --from=builder /out/claude_plays      /usr/local/bin/terminalgb-agent

# The vendored MIT dmg-acid2 ROM (third_party/dmg-acid2/LICENSE travels with
# it), so the image boots something real with nothing configured. This is the
# ONLY game content in the image and the only game content there will ever be:
# no commercial ROM is shipped, fetched, or accepted from a build argument.
COPY third_party/dmg-acid2/dmg-acid2.gb    /opt/terminalgb/roms/dmg-acid2.gb
COPY third_party/dmg-acid2/LICENSE         /opt/terminalgb/roms/dmg-acid2.LICENSE

# The agent adapters: stdlib-only Python that turns a state snapshot on stdin
# into a plan on stdout. The API key is read from the environment at run time
# and is never baked in here — see docs/quickstart.md §"The key".
COPY tools/claude_plays/                   /opt/terminalgb/agent/
COPY tools/oneclick/terminalgb-run         /usr/local/bin/terminalgb-run

ENV TERMINALGB_ROM_DIR=/opt/terminalgb/roms \
    TERMINALGB_AGENT_DIR=/opt/terminalgb/agent \
    TERM=xterm-256color

USER gb
WORKDIR /home/gb

# No default ROM argument: `terminalgb-run` picks the vendored one when the user
# names none, and says so.
ENTRYPOINT ["/usr/local/bin/terminalgb-run"]
CMD ["play"]

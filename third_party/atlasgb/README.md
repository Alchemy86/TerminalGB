# AtlasGB (vendored)

The Gen 1 memory atlas: every byte of Pokémon Red/Blue's work RAM and high RAM
accounted for, indexed by address and by name, each entry carrying the evidence
for it rather than a transcription of somebody else's table.

* **Upstream:** <https://github.com/Alchemy86/AtlasGB>
* **Licence:** MIT. The addresses and structure layouts are facts and the symbol
  names are [`pret/pokered`](https://github.com/pret/pokered)'s; upstream's
  `docs/licensing.md` is the honest longer answer, and it is a better one than
  "MIT" alone.
* **What is vendored:** `pokemon-rb/atlas.tsv` — the source of truth, tab
  separated, 2,898 entries. The pinned atlas id, ref, commit and checksum are in
  `pokemon-rb/atlas.lock`.

AtlasGB is an atlas **project**, not one atlas — Pokémon Red/Blue is its first —
so the vendored copy is namespaced by atlas id the same way upstream is. A
second atlas costs `tools/fetch-atlas.sh --atlas <id>` and no restructuring.

## Why it is here and not in `docs/`

The atlas started in this repository and moved out, and the move is the point.

An atlas kept inside one emulator is that emulator's private note. Out of tree
it can be adopted by a save editor, a randomiser, a debugger or another emulator
without any of them taking a dependency on us — which is what a source of truth
has to be able to be. It also separates two jobs that were tangled here: AtlasGB
owns the *data* and everything provable from the file alone, and this repository
owns the *evidence*, because proving an address against a running cartridge
needs a cartridge and an emulator and upstream has neither.

Vendored rather than fetched at build time and rather than a submodule: a
submodule breaks shallow checkouts and source tarballs and makes a one-file
dependency cost a clone, and a build-time fetch would put the network on the
path of `cargo test` — fatal here, because the atlas's structural checks and the
anti-drift gate are supposed to run on every plain test run. Same shape as
`third_party/dmg-acid2/` beside it, plus the one thing that frozen artefact does
not need: a lock, because this one is alive and will be re-released.
`third_party/gbselftest/` beside it is vendored the same way. Upstream's
`docs/consuming.md` asks for exactly this.

## Using it here

```sh
tools/fetch-atlas.sh --verify           # offline: is the copy the pinned one
tools/fetch-atlas.sh --check-upstream   # advisory: has upstream moved on
make test-gen1-atlas                    # the harness that reads it
```

[`testharness/gen1atlas.rs`](../../testharness/gen1atlas.rs) reads this file. It
makes the same checksum comparison `--verify` does, in-process and with no
subprocess, so a plain `cargo test --release` catches a hand-edit too.

**Do not edit anything in this directory**, and in particular do not edit the
`verify` column: upstream makes it unwritable by hand on purpose — it is
rewritten by landing a verification report and upstream's CI checks a digest of
it. A hand-edited tier is a claim with nothing behind it, which is the failure
mode the whole project is a reaction to. Publish evidence back instead:

```sh
make atlas-evidence POKE_ROM=~/roms/pokeblue.gb
```

and open a pull request upstream that runs `tools/apply-evidence.py report.json`.

## Refreshing

```sh
tools/fetch-atlas.sh --ref <tag-or-commit>
make test-gen1-atlas
```

A refresh moves what the harness scores, so it belongs in a commit of its own
with the harness's output in the message.

**The vendored file is the atlas's own header-row TSV, with no preamble.** That
is upstream's shape and it is deliberate: GitHub renders a TSV whose first line
is the header as a table, and a comment banner above it turns the whole file
back into plain text. Do not reintroduce one when re-pinning.

For the record, because it is the kind of thing that reads as suspicious later:
this was first pinned at the pre-namespace path `data/atlas.tsv` while upstream
was mid-restructure, and re-pinned here once that landed. **The bytes did not
move** — same `dc07eeec…` before and after, git recorded a pure rename — so
nothing this repository verifies ever depended on which pin was in force.

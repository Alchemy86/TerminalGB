# The Gen 1 memory atlas — it lives at AtlasGB now

[← Gen 1 area](../README.md) · **[AtlasGB](https://github.com/Alchemy86/AtlasGB)** ·
[by address](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-address.md) ·
[by name](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md) ·
[the structures](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/structures.md)

**Every address in Pokémon Red and Blue, what lives there, and the evidence that we watched
the cartridge use it.** It was built here and it is now its own project:
**<https://github.com/Alchemy86/AtlasGB>**.

This page is a pointer, deliberately, and not a second copy. Twenty-two generated pages and
a 238 KB data file used to sit in this directory; a mirror that nothing regenerates is a
mirror that is wrong within a month, and there is no version of "two sources of truth" that
is better than one.

---

## Why it moved

An atlas kept inside one emulator is that emulator's private note. Out of tree it can be
adopted by a save editor, a randomiser, a debugger, a bot or another emulator without any of
them taking a dependency on us — which is what a source of truth has to be able to be. It
also separates two jobs that were tangled together in this repository.

**AtlasGB owns the data**, and everything provable from the file alone: that it parses, that
every address is inside the region it claims, that the entries march upwards, that work RAM
and high RAM are accounted for byte for byte, that every link and anchor resolves. Its CI
runs all of it on every push, with no cartridge anywhere in sight.

**This repository owns the evidence**, because that is the half upstream cannot do for
itself. Proving an address needs a running cartridge, and AtlasGB has neither a cartridge nor
an emulator. So the evidence tiers — *the cartridge names it*, *it changed while the game
ran*, *a named invariant proved it means what it says* — are produced by
[`testharness/gen1atlas.rs`](../../../testharness/gen1atlas.rs) here and **published back**
as a report. Upstream then holds its `verify` column to a digest of that run, so a tier in
the published atlas always names a real run against a real cartridge rather than somebody's
recollection.

---

## How this repository uses it

A **pinned snapshot** is vendored at [`third_party/atlasgb/`](../../../third_party/atlasgb/README.md)
— one committed file plus a lock recording the atlas, the ref, the commit and a sha256. Not a
submodule (it breaks shallow checkouts and source tarballs and makes a one-file dependency
cost a clone) and not a build-time fetch (it would put the network on the path of
`cargo test`, which is fatal when the checks are supposed to run on every plain test run).

AtlasGB is an atlas **project**, not one atlas — Pokémon Red/Blue is its first — so both the
vendored copy and the fetch script are namespaced by atlas id (`--atlas pokemon-rb`, the
default). A second atlas would cost a flag, not a restructuring.

```sh
tools/fetch-atlas.sh --verify           # offline: is the copy still the pinned one
tools/fetch-atlas.sh --check-upstream   # advisory: has upstream moved on
make atlas-upstream                     # the same, as a target
make test-gen1-atlas                    # the harness that reads it
```

The harness makes the same checksum comparison in-process, so a plain
`cargo test --release` catches a hand-edited copy without shelling out to anything.

### The anti-drift gate

The check worth copying even if you take nothing else, and the reason the snapshot is
vendored rather than fetched: **every Gen 1 address hard-coded anywhere in this source must
appear in the atlas.** It is a text scan for `const NAME: u16 = 0x____;` across a named list
of files, and it runs in Tier A with no cartridge and no network — so drift is caught on the
commit that introduces it.

It has already earned its place twice. `battle_struct` was 29 bytes in one plugin and 28 in
another, and the atlas settles it — `wTrainerClass` sits immediately after `wBattleMonPP + 4`.
And `wCurMap` was written out in nine separate files with nothing able to say whether they
agreed.

A new consumer joins the gate by being named in `PLUGIN_SOURCES`. Forgetting to add a file is
exactly what the gate exists to make visible, so adding one is a one-line change on purpose.

### Publishing evidence back

```sh
make atlas-evidence POKE_ROM=~/roms/pokeblue.gb    # -> target/atlas-evidence.json
# then, in a clone of AtlasGB:
tools/apply-evidence.py target/atlas-evidence.json && make docs data
```

**Measured, and it is the check to repeat after any change to this machinery:** landing our
report upstream moves **0 entries** and reproduces the published `verify_digest`
`sha256:59c413f2ff645ff3…` exactly. That is what says the evidence this repository produces
is the evidence the published atlas carries, rather than something that merely resembles it.

The report is `atlasgb-evidence/1` and carries **every symbol in the atlas**, including the
ones that earned nothing — upstream refuses a partial report rather than guessing, because an
omission would read as a downgrade. It is written only when `ATLAS_EVIDENCE_OUT` names a
path: publishing evidence is a deliberate act, and a file that appeared on every test run is
a file somebody commits by accident.

**The `verify` column is not re-blessed here.** It is the one baseline in this repository
that is not ours to rewrite, and the lock check would catch it if somebody tried. That is the
design, not an inconvenience: an evidence tier nobody can audit is the thing AtlasGB exists
not to be.

---

## Where to read it

Upstream splits its pages in two: the ones about **the project** stay at `docs/`, the ones
about **this atlas** live under `atlases/pokemon-rb/docs/`. Anchors are unchanged —
`#s-<symbol>` and `#a-<ADDR>` — so a link to a symbol keeps its fragment.

| | |
|---|---|
| [AtlasGB](https://github.com/Alchemy86/AtlasGB) | the project — what it is, how it is built, what is and is not claimed |
| [by address](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-address.md) · [by name](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/by-name.md) | the two indexes over all 2,898 entries |
| [the structures](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/structures.md) | `party_struct`, `battle_struct`, `box_struct` and the rest, written out field by field |
| [the schema](https://github.com/Alchemy86/AtlasGB/tree/main/docs/schema.md) | what the ten columns mean — read this before writing a parser |
| [verification](https://github.com/Alchemy86/AtlasGB/tree/main/docs/verification.md) | the four evidence tiers, the invariants, and the loop this repository closes |
| [consuming it](https://github.com/Alchemy86/AtlasGB/tree/main/docs/consuming.md) | the contract this page implements, if you want to do the same in your own project |

> **Why the chapters sit under `atlases/pokemon-rb/` and the rest do not.** AtlasGB started
> flat, holding one atlas; when it became an atlas *project* the pages about the project
> stayed at `docs/` and the pages about *this* atlas moved under the atlas that owns them,
> keeping their filenames and their fragments. `third_party/atlasgb/pokemon-rb/atlas.lock`
> records which upstream state this repository is pinned to.

The [memory map](../memory-map.md) beside this page has **not** moved and is still the right
page for *why*. It is task-shaped — the party, the battle, the screen, the map, the dice —
and it explains mechanisms. The atlas is the *where*.

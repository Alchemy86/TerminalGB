# Building `pokegold` so its symbol table is exact

[← Gen 2 area](README.md)

**Every address in this area has one grade, and that grade is a claim about a build.** If the
build is not byte-identical to the cartridge, a symbol's address is a guess dressed up as a
fact. So this is the first page, not an appendix: it is the thing that makes the rest
checkable.

It is also about an hour of work, and it has been done once here already.

---

## 1. What "exact" means, and why it is achievable

A disassembly's symbol table (`pokegold.sym`) maps every label to the address the *assembler*
placed it at. That address is only the address in *your* cartridge if the assembler produced
the same bytes the cartridge holds. `pret` projects make that checkable: each ships a
`roms.sha1` listing the checksums its build is expected to reproduce.

**Measured here:** `pret/pokegold` built at RGBDS **1.0.3** produces a Gold ROM with SHA-1
`d8b8a3600a465308c9953dfa04f0081c05bdcb94`, which matches the entry in the disassembly's own
`roms.sha1` and matches the maintainer's cartridge. So `pokegold.sym` is exact for it, and every
symbol-file address on these pages inherits that.

The same is true of `pret/pokered` for Red and Blue — the Gen 1 area's addresses rest on the
identical argument ([`docs/gen1/README.md`](../gen1/README.md#standards-for-this-area)). It is
one technique, applied twice.

---

## 2. The toolchain version is not a detail

**RGBDS 1.0.3.** Measured here: **0.6.1 is too old** — the repository demands ≥ 1.0.0. The
prebuilt Linux tarball from the gbdev releases is the quickest route; nothing needs to be built
from source.

A too-old toolchain announces itself. The failure mode to actually fear is a *different* version
that assembles cleanly and places a section a few bytes elsewhere, which produces a symbol table
that is wrong in a way nothing complains about — and which is precisely what the `roms.sha1`
check catches. **Never skip the checksum step because the build succeeded.**

---

## 3. Deriving a constant, rather than counting one

Half the numbers this area needs are not addresses at all — they are `EQU` constants and event
indices (`PARTYMON_STRUCT_LENGTH`, `JOHTO_POKEMON`, `EVENT_MET_BILL`, `DAILYFLAGS1_TIME_CAPSULE_F`).
Those are computed by the assembler through `const_def` / `const` chains that can be hundreds of
lines long, and counting them by hand is how you get a number that is off by one and looks
right.

Ask the assembler instead:

```
rgbasm -Q8 -P includes.asm -D _GOLD
```

with a `PRINTLN "{d:SYMBOL}"` for each constant you want. `-D _GOLD` selects Gold over Silver,
and `-P includes.asm` pre-includes the file that pulls in the constant definitions, so the
assembler has every `const_def` chain in scope.

This is the same discipline as reading an address out of the cartridge rather than trusting a
table: **let the machine that computes the number tell you the number.** Every constant in
[`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs) came out of this
invocation, not out of a hand count.

---

## 4. What to keep, and what not to

**Transcribe the addresses you need, with their symbol names, into the code that uses them.**
That is what this repository does: each constant is a `const` with the symbol name and the
routine that reads it in a doc comment, so a reviewer can check any one of them against the
disassembly in ten seconds.

**Do not vendor the symbol file wholesale.** Two reasons, and the second is the stronger one:

1. It is a build artifact of somebody else's project, and this repository's standing rule is
   that facts and formats are ours to document while other people's material is not ours to
   redistribute (see the licence-boundary section in `CLAUDE.md`).
2. A wholesale copy makes every address *uncited*. The value of the transcription discipline is
   that it forces each number to arrive attached to a name and a reason. A 20,000-line symbol
   dump in the tree would let somebody grab an address with no idea what reads it, which is how
   plausible-looking Gen 2 numbers get into a tool.

The build is reproducible in an hour. The citation is what does not regenerate itself.

---

## 5. What this does *not* give you

**A symbol file is exact for the cartridge it was checked against, and says nothing about any
other.** Gold's symbols are not Silver's until somebody builds Silver and checks it, and they
are emphatically not Crystal's. See [which cartridge](README.md#which-cartridge).

**It also gives you no run-time protection.** A tool holding a transcribed address will happily
write it into whatever cartridge it is handed. Gen 1 tooling here closes that by locating its
write addresses in the ROM image at attach time and refusing a cartridge that does not match;
**no Gen 2 code does this**, and it is named as the highest-value gap in
[the area README](README.md#what-is-not-established-about-gen-2-here).

---

## What this suggests next

* **Build Silver the same way** (`-D _SILVER`) and diff the two symbol tables. If they agree on
  everything this area names, one line on every page changes from "not checked" to a second
  cartridge covered, for an hour's work.
* **Build Crystal** and diff *that*. The interesting output is not the addresses but the list of
  symbols that moved, because it says exactly how much of this area is Gold-specific rather than
  Gen-2-generic — which nobody currently knows.
* **Generate the transcription instead of writing it.** The constants already come out of
  `rgbasm`; emitting a Rust file from that output would make the citation mechanical and let a
  build check itself against a fresh symbol table.

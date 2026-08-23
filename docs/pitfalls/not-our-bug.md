# When it is not your bug

← [Pitfalls index](README.md)

The most expensive class of all, because you can spend weeks correctly fixing a thing that was
never wrong. Three sources: the cartridge, the suite's denominator, and the reference.

---

## The room really has no door: checking the game's code first

**Symptom, in a player's words.** *"There is simply no leaving the damn room. Blue is trapped
in the void trade room. Gold has a door to walk out."*

**What it looks like.** A link bug in the emulator — a cancel that is not reaching the far
end, a protocol state machine that has wedged, a room whose exit the emulator failed to draw.
The asymmetry between the two games makes it worse: one cartridge behaves and the other does
not, which is exactly what an emulator bug in a shared code path would look like.

**Root cause: three separate claims with three different answers, and none of them is ours.**

1. **One player cancelling is not enough, by design.**
   `.cancelMenuItem_APressed` (`pokered/engine/link/cable_club.asm`) writes `$F` to the
   exchange nybble and requires `$F` back before it falls through to `ReturnToCableClubRoom`.
   The disassembly comments the compare itself: *"did the other person choose Cancel too?"*
   A player whose partner is still browsing presses A on CANCEL for ever — while the game
   keeps rendering, the music keeps playing and the clock keeps running. It is not a hang and
   it does not look like one.

   Generation 2 is the same rule in Gen 2's names: `LinkTradeOTPartymonMenuCheckCancel`
   (`pokegold/engine/link/link.asm`) puts `SERIAL_TIMECAPSULE | $F` = `$6F` on the wire —
   byte for byte what Gen 1 sends — and requires `$F` back. **So there is nothing for an
   emulator in the middle to relay or translate.** Both consoles are speaking the protocol
   correctly and are simply waiting for their players.

2. **Both cancelling does leave the trade screen** — and only the screen.

3. **The room itself has no exit tile.** `TradeCenter`'s object file declares
   `def_warp_events` with *no entries*, so `wNumberOfWarps` reads **0** at run time (read out
   of the running game, not inferred). `ReturnToCableClubRoom` reloads the same map. The
   Colosseum is the same. Gold's Time Capsule really does have doors —
   `TimeCapsule_MapEvents` (`pokegold/maps/TimeCapsule.asm`) declares two `warp_event`s back
   to `POKECENTER_2F`, one behind each seat. **That asymmetry is two cartridges, not a bug.**

**Resolution.** The exit the cartridge ships is **START → RESET**. `StartMenu_SaveReset`
(`pokered/engine/menus/start_sub_menus.asm`) jumps to `Init` whenever `BIT_LINK_CONNECTED` is
set, which is why the START menu in that room shows RESET where it normally shows SAVE. It is
not a warning label; it is the door. Measured safe: RESET → CONTINUE returned the player to
the Pokémon Center it had saved in, party intact, with neither console faulting — so no
battery was poisoned.

**Side effects and blast radius.** The right response to "the cartridge does this" is not
always "do nothing". A room with no exit and no explanation reads as *the emulator has trapped
me*, so the product now watches for the Cable Club and says both obstacles out loud. **That is
a UI change, not an emulation change** — the emulated machine is byte-for-byte what it was,
which is the property you must preserve when you decide to help.

**How we found it.** Two whole cores on one cable, so nothing of ours was impersonating
anything: 60 A presses on CANCEL with the link state pinned, the exchange nybble latched at
`$0F`, and the cancel cursor cell showing the hollow `▷` — the observable signature of
"pressing A on CANCEL for ever". Full write-up, with every address and how it was graded:
[`docs/gen1/leaving-the-cable-club.md`](../gen1/leaving-the-cable-club.md).

> **The general rule.** Before fixing "a bug in the emulator", read the game's own code. For
> the Pokémon titles the disassemblies (`pret/pokered`, `pret/pokegold`) settle most questions
> in ten minutes, and they are the *authority* — cite by **symbol name**, never by bare hex
> address, because a symbol survives across revisions and a bare address is wrong for the next
> game and unsearchable for the next person.
>
> The corollary that bit us: **a prose note in your own docs that contradicts the disassembly
> sends the next person hunting a defect that does not exist.** This repository's own notes
> asserted the opposite of point 3 for months, unmeasured. Check the claim against the
> cartridge before building on it.

---

## Most of a suite's remaining rows can be unwinnable

**Symptom.** A picture-judged suite sits at a low score and refuses to move however correct
the emulator gets. Today: <!--score:mealybug_accurate-->29 / 79<!--/score--> on the per-dot
engine.

**What it looks like.** *Everything that is not passing* is work remaining, and an emulator
that is a long way from correct. Subtracting one score from the other is the trap.

**Root cause.** A characterisation of every failing row — by the crew that owns
[`docs/mealybug.md`](../mealybug.md) — found that **42 of the (then) 53 still failing cannot be
made pixel-exact by any PPU work at all**, for three unrelated reasons:

| group | rows | why not |
|---|---:|---|
| the boot ROM's `®` glyph | 27 | nine ROMs place objects whose tile is `$19`, the `®` the DMG boot ROM leaves in VRAM. We do not ship that glyph — a register value is a fact about hardware, a **glyph bitmap is expressive content**, and this repository must stay freely distributable |
| colour references | 8 | those references carry 125 distinct luminances (a Color console colourising a DMG cartridge from its boot ROM's title-hash palette) against four or fewer everywhere else |
| `*2` ROMs | 7 | the shipped ROM and its reference are **different machines** |

So the reachable ceiling in CI is 79 − 42 = **37**, and at
<!--score:mealybug_accurate:pass-->29<!--/score--> passing the rows actually in play are
**eight**, not the eleven this paragraph quoted while the passing count still read 26 — a
worked example of its own point: **re-derive `ceiling − passing` from the wrapped score every
time you read this**, never keep the subtraction's *result* as prose, or it goes stale exactly
like this did. The ceiling itself is the number that does not move.

**And the first cut of that table was itself half a row-group wrong**, which is worth knowing
before trusting any such classification: `grep -l '$19'` finds **ten** ROMs and only nine are
blocked, because `m3_wx_4_change_sprites` places its objects with that tile and then fills
every tile with `$FF` before it starts, so the glyph never reaches its screen. Those three rows
were counted as unwinnable and are pixel-exact now. **Referencing the blocker is a first pass,
not a verdict** — check whether the ROM overwrites it.

**Resolution.** Publish the denominator that hurts *and* the one that is honest, and say which
is which. "53 rows to go" is the wrong number; "eleven rows are in play, 42 are structurally
out of reach and here is why each is" is the right one. Note also that the unwinnable rows
**still repay work** — their residual after the missing glyph is real mid-scanline error, and it is
the same error as the winnable groups.

**Side effects and blast radius.** Getting the denominator wrong in either direction is
costly. Overstating it demoralises and misdirects; understating it (quoting only the
winnable subset as if it were the suite) is the flattering error and is worse. The same
question has a different answer for every consumer — Mooneye `acceptance` is 75 rows in this
repository's baseline and **66** in both the community's usual denominator and the GB
Emulator Shootout's manifest, because the Shootout comments out all fifteen model-variant
ROMs. A commit that moved eleven real rows here moved the Shootout number by **exactly zero**,
and that was correct rather than a measurement error. See
[`docs/measured/scoreboard.md`](../measured/scoreboard.md).

**How we found it.** By characterising every failing row's signature before scoping the work
— see [techniques.md](techniques.md#characterise-failures-before-you-fix-them).

---

## The reference images are not hardware photographs

**Symptom.** Nothing observable. This is a **provenance** error, and its only symptom is that
you reason about the wrong oracle.

**What it looks like.** Confidence. "The reference was photographed off real hardware" is a
much stronger claim than the one the suite actually makes, and it changes what a disagreement
means.

**Root cause.** The Mealybug Tearoom Tests README says it plainly, and it is worth quoting
because our own docs did not:

> You can check in the `expected` directory for screenshots from **my Game Boy emulator**
> (which I believe to be correct), and the `photos` directory contains **blurry photos** of
> the ROMs running on real devices.

The `expected/` PNGs — the ones every automated runner compares against, ours included — are
the output of the suite author's own emulator (Beaten Dying Moon), cross-checked by him
against the photographs in `photos/`. That is still an excellent oracle. It is not the same
claim.

**Resolution.** Say what it is. And keep in mind what follows from it: `expected/` and
`photos/` are two different artefacts, and where a row is genuinely contested the photographs
are the tiebreak, not the PNGs.

**Side effects and blast radius.** None mechanically — no score moves. It matters for
*reasoning*: the "different machines" group above only makes sense once you know the
references are emulator output for a stated machine rather than a photograph of whatever was
plugged in.

**Status.** `docs/mealybug.md` and the header of
`testharness/mealybug_accurate_baseline.txt` still describe the references as
hardware-photographed. Those files belong to another crew and are theirs to correct; this
entry records the verification (the suite's own README, read from the pinned checkout in
`target/test-suites/mealybug/`) so the correct claim is at least written down somewhere.

---

## Two suites disagreeing about one address is a statement about two machines

**Symptom.** Closing rows in one suite makes another suite's distance jump.

**What it looks like.** A regression, or a rule you got backwards.

**Root cause.** The worked example is the block Nintendo calls prohibited,
`$FEA0-$FEFF`. Pan Docs, ["FEA0–FEFF range"](https://gbdev.io/pandocs/Memory_Map.html), gives
it **three** answers: `$00` on DMG/MGB/SGB/SGB2, "a unique RAM area, masked with a
revision-specific value" on CGB revisions `0-D`, and the high nibble of the low address byte
twice (`$FEAx` → `$AA`) on CGB revision E, AGB, AGS and GBP. Gambatte's Color reference console
is **CPU CGB C**, so its rows pop back whatever they wrote there. `cgb-acid-hell`'s
hardware-photographed reference does not.

**Resolution.** Measure both, and pick with the numbers on the table rather than by picking the
suite you were working on:

| | Gambatte | `cgb-acid-hell` |
|---|---:|---:|
| the block reads `$FF` (what we had) | 3083 | 2 px |
| the revision E / AGB nibble rule | **3109** | **2 px** |
| revisions `0-D`, RAM with no mask | **3135** | **176 px** |

We take the revision E rule. Three reasons, all of them checkable: the emulator already targets
CGB-E elsewhere, two pixels from a hardware photograph is worth more than 26 rows measuring
another stepping, and **"RAM with no mask" is a guess where the nibble rule is documented** —
Pan Docs gives no mask for any revision in `0-D`.

**Side effects and blast radius.** The general form is uncomfortable and worth stating: *a
suite's score is a statement about the machine its references came from.* When two suites give
one address two answers, at most one of them is describing the console you model, and the
honest write-up names which machine each of them is rather than quietly optimising the total.

**How we found it.** Implementing the RAM behaviour, running every suite, and reading the one
row that moved the wrong way. `make test-external` scoring twelve other suites is what made
that a five-minute check instead of a silent regression.

---

## More than one reference image means more than one machine

**Symptom.** A ROM fails against one reference set and passes against another, or a row is
red by a distance that exactly equals the gap between two of the suite's own references.

**What it looks like.** An emulator bug.

**Root cause.** When a suite ships several references for one ROM it is telling you the answer
differs by machine. Mealybug ships three (DMG-blob, CPU CGB C, CPU CGB D); SameSuite's
`-cgb0` and `-cgbB` ROMs **contradict each other**, so no single machine can pass both.
Scoring several references against one machine is a **runner** bug that no emulator can pass.

**Resolution.** Read the machine off the ROM's filename or the reference set's name and boot
*that* machine. Ours does: the Mealybug runner runs three machines, one per reference column;
the SameSuite runner maps a `-cgb0`/`-cgbB`/`-cgbC` suffix to a CPU revision; the ROM harness
runs every Blargg ROM on both consoles as `<id>@dmg` / `<id>@cgb`.

**Side effects and blast radius.** Introducing a machine axis means the emulator now models
console *revisions*, which is real work — but the alternative is a permanently unreachable
score. Note that a suffix naming **several** machines (`-GS`, `-cgbABCDE`) must be left alone:
forcing one of them boots the wrong console for the rest.

**The diagnostic that closes rows in minutes.** Compare the suite's own reference sets against
each other. Two of ours were failing by 138 and 864 pixels — and the C-versus-D references for
those ROMs differ by exactly 138 and 864. **If the gap between two references equals your
error, the emulator is right and the runner is wrong.**

---

## An honest suite has deliberate reds

**Symptom.** A red row that somebody keeps trying to fix.

**What it looks like.** An outstanding bug.

**Root cause.** Some behaviour is genuinely absent from the machine you are emulating.
`blargg/oam_bug@cgb` is pinned **FAIL** here, because the DMG OAM-corruption bug is a defect
of DMG/MGB/SGB silicon that Game Boy Color hardware does not have — even running a monochrome
cartridge (Pan Docs, *"OAM Corruption Bug"*). **A real CGB fails this ROM too.** Turning that
row green would mean emulating a machine nobody has.

The same shape appears in SameSuite (`channel_*_extra_length_clocking-cgb0B` asserts the
behaviour of CPU CGB revisions ≤ B, which the ROM's own source calls a bug that *"breaks at
least one game (Prehistorik Man)"*) and in the model-variant Mooneye rows for consoles we do
not model.

**Resolution — how to record one so nobody "fixes" it.** Three things, and all three are
needed:

1. **The expectation file states the expected result explicitly.** `testharness/baseline.txt`
   records `FAIL` for that row, not an absence.
2. **The reason sits directly above it, in the same file**, in prose, with a citation. Not in
   a commit message, not in a wiki — in the file the next person will open when the row goes
   green unexpectedly.
3. **The gate is red on a change in *either* direction.** A row that starts passing is a
   baseline change too, and must be argued for, not absorbed.

**Side effects and blast radius.** This is what makes a suite's number meaningful in both
directions. Without (3), an emulator can drift *into* passing a test it should fail and nobody
will ever ask why.

**How we found it.** It was designed rather than found — but the design is the product of the
[self-judging harness](tests.md#a-harness-whose-expected-output-is-its-own-output) that
preceded it.

---

## A red check that never ran is a billing failure, not a test failure

**Symptom, verbatim.** Every check on a pull request is red, and each one reports the same
sentence instead of a log:

```
The job was not started because recent account payments have failed or your
spending limit needs to be increased. Please check the 'Billing & plans'
section in your settings
```

**What it looks like.** A repository-wide breakage. Seven checks failing at once across two
unrelated workflows — the lint job, the embedding build, the test-ROM harness, the external
suites, both conformance scores and the self-test cartridge — reads as "the branch broke
something fundamental", and the fundamental thing every one of those jobs has in common is the
core building at all. The wrong conclusion is that the core does not compile.

**Root cause: no job ran.** GitHub reports a refusal to *start* through the same red tick it
uses for a failure, and the check name is all a reviewer sees on the pull-request page. Every
one of those gates was green on the same commit locally: `make lint`, `cargo test --release`,
the test-ROM harness, `make test-external` and `make test-external-accurate` in both picture
modes, `make gbselftest`, and the conformance runner on both the submodule and the fetched-ROM
path. Nothing in the tree was wrong.

**The repository's own contribution to it, which is the part worth fixing.** Every workflow
that runs on a branch was declared `on: [push, pull_request]`. For a branch in this repository
with a pull request open, that fires **both** events for the same commit, so every push billed
two identical runs of everything — and "everything" here is not cheap: the external-suites job
alone fetches a few hundred megabytes of ROMs and scores several thousand rows, and the
embedding job runs the core under qemu on three further architectures. Confirmed rather than
assumed, from the run list: one commit, two `CI` runs and two `Conformance (self)` runs,
distinguished only by `event=push` against `event=pull_request`.

**Fix it across every workflow file, not just the ones whose checks are red — the expensive
one hid.** `ci.yml` and `conformance-self.yml` are the two that appear on the pull-request page,
so they are the two a reviewer sees and the two that get fixed. `container.yml` is
**`paths:`-filtered**: it runs only when `Dockerfile`, `.devcontainer/**`, `tools/oneclick/**`
or `Cargo.{toml,lock}` change, so it is absent from most pull requests and easy to miss
entirely — and it is the costliest job in the repository when it does fire, building the whole
Rust toolchain image twice over and pushing a `mode=max` layer cache each time. It carried the
same duplicate for a whole round after the other two were fixed. **`grep -l 'pull_request'
.github/workflows/*.yml` is the sweep**; a workflow that never runs on a branch (`workflow_call`,
a tag, `workflow_dispatch`) is the only one that can be left alone.

**Resolution.** Limit `push` to `main` (nothing opens a pull request against it, so it would
otherwise lose coverage entirely) and let `pull_request` cover everything else exactly once;
then add a `concurrency` group keyed on the workflow and the ref so pushing a fix to a branch
cancels the superseded run instead of paying for both. `main` is excluded from the cancelling,
because its runs are the published record. The explanation lives in the workflow files
themselves, which is where somebody wondering why a branch push produced no run will look.

**Side effects and blast radius.** Coverage is unchanged: every commit that reaches `main`
still runs every workflow, and every pull request still runs every workflow it is eligible for
(`container.yml` keeps its path filter on both events, so it stays absent from a pull request
that touches none of those files). What is lost is the second, identical run of a branch push,
and any run already superseded by a newer push to the same branch. A branch pushed with **no**
pull request open now gets no run at all — that is the deliberate part of the trade, and it is
the right way round for a project that develops on local branches and opens a pull request when
it wants the gates.

**How we found it.** By reading the run list rather than the check names:
`gh api "repos/<owner>/<repo>/actions/runs" --jq '.workflow_runs[] | "\(.id) \(.name)
event=\(.event) sha=\(.head_sha[0:7])"'` prints the event beside the commit, and the duplicate
pairs are then obvious. **Do this before debugging a wall of red checks**: if the runs have no
logs, there is nothing in the code to find.

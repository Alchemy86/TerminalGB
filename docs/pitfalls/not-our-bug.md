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
the emulator gets. Today: <!--score:mealybug_accurate-->20 / 79<!--/score--> on the per-dot
engine.

**What it looks like.** *Everything that is not passing* is work remaining, and an emulator
that is a long way from correct. Subtracting one score from the other is the trap.

**Root cause.** A characterisation of every failing row — by the crew that owns
[`docs/mealybug.md`](../mealybug.md), measured mid-campaign with 22 rows passing and 57
failing — found that **45 of those 57 cannot be made pixel-exact by any PPU work at all**, for
three unrelated reasons:

| group | rows | why not |
|---|---:|---|
| the boot ROM's `®` glyph | 30 | ten ROMs place objects whose tile is `$19`, the `®` the DMG boot ROM leaves in VRAM. We do not ship that glyph — a register value is a fact about hardware, a **glyph bitmap is expressive content**, and this repository must stay freely distributable |
| colour references | 8 | those references carry 125 distinct luminances (a Color console colourising a DMG cartridge from its boot ROM's title-hash palette) against four or fewer everywhere else |
| `*2` ROMs | 7 | the shipped ROM and its reference are **different machines** |

So the reachable ceiling in CI is 79 − 45 = **34**, and at 22 passing the rows actually in play
were **12**. (The gated baseline on this branch is
<!--score:mealybug_accurate-->20 / 79<!--/score-->; that crew's work is in flight. The ceiling
is the number that does not move.)

**Resolution.** Publish the denominator that hurts *and* the one that is honest, and say which
is which. "62 rows to go" is the wrong number; "12 rows are in play, 45 are structurally out
of reach and here is why each is" is the right one. Note also that the unwinnable rows **still
repay work** — their residual after the missing glyph is real mid-scanline error, and it is
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

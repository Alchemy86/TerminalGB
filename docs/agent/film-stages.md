# The chain film shows the walking and hides the story

The maintainer: *"The journey makes no sense to me. We're supposed to be building up in stages, so where
are the clips from, say, in the lab with Oak? Like what his videos do. Did we have some larger goal
that just said 'fuck off north to town'?"*

**He is right, and it is structural, not cosmetic.**

## The diagnosis

**The film is a map view, and half the chain does not happen on a map.**

Oak's lab is its own map. The player's house is its own map. The Mart interior is its own map. **A
battle has no map position at all**, and neither does a dialogue box. So every stage beat —
taking the starter, the rival battle, collecting the parcel, receiving the Pokédex — is **invisible by
construction.** What survives is the outdoor walking, which is why 14 staged links read as "go north".

**This is exactly why Whidden did it two ways**, and it is in this project's own original swarm brief:

> 1. **Sprites on a stitched world map** — each agent at its map/x/y.
> 2. **A collage of actual Game Boy screens**, each placed at that agent's world position, overlapping
>    where agents are near each other. Some show the overworld, some a battle, some a menu — every one
>    a real framebuffer.

**#2 was marked a stretch goal and never built. Build it now** — it is the only thing that can show a
lab, a battle, or a conversation, and it is the difference between a walking simulator and the story.

## What to build

**Composite real framebuffers, not just markers.** For each agent, place its actual 160x144 screen at
its world position — the raw-shot path (`docs/picture.md`) already gives an un-upscaled, un-palette-
mapped framebuffer, and the swarm compositor already knows where every agent is.

Then the indoor and battle moments appear **because the agent's own screen shows them**, with no
special-casing per map.

**Make the stage structure legible.** The chain has 14 links; the film should say which link each
agent is on and how far the field has progressed. A per-link progress strip, or grouping/colouring
agents by their current link, turns "agents on a map" into "a field advancing through stages" — and it
fills the black bars that are currently wasted.

**And mark the beats.** When an agent takes the starter, wins the rival battle, or receives the
Pokédex, that is an event flag firing — **we know the exact frame.** Call it out on screen. Those are
the moments the whole chain exists to reach and right now they pass unremarked.

## Keep what already works

- The 52x timelapse and the HUD (agents / emulated minutes / compression) are good — keep them.
- The character sprites with numbered rings are good — keep them.
- The every-attempt film is the better artefact of the two **because it has one clear subject and
  states its own method on screen.** Apply that discipline here: the chain film should say what it is
  showing and what run produced it.

## Do not

- Do not re-simulate. The 24 tracks and 1.2M decision rows are on disk.
- Do not fake continuity. If an agent is indoors and its screen is all that can be shown, show that —
  **an honest cut to a real framebuffer beats an invented overworld position.**

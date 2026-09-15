# A Charizard from the cartridge, held on E Ink

Six inks, 400x600, and a picture that costs nothing to keep. Pull the USB lead
out and it is still there.

```sh
pixelgb species --rom ~/roms/blue.gb --out /tmp/species
TERMINALGB_SPRITE_PNG=/tmp/species/pokemon-blue/sprites/four-shade/1x/pokemon-front-006-charizard.png \
  tools/m5paper-build.sh charizard
```

Button **C** cycles the look. Nothing else on the device does anything, which is
the point — `tools/m5paper-build.sh flash` still puts the running emulator back.

## The pixels are the cartridge's

Charizard's front sprite is 56x56 pixels of run-length-and-delta compressed data
in bank `$0D` of a Pokémon Blue image. `pixelgb species`, a sibling project,
decompresses it. Checked against `pret/pokered`'s own `gfx/pokemon/front/
charizard.png` — the artist's original file, which the data in the cartridge was
compiled *from* — **0 of 3,136 pixels differ**. Its shade histogram is
1,437 / 603 / 567 / 529 on both sides, lightest first.

That is the whole of the sprite's provenance. Nothing was drawn, traced or
fetched from the internet.

Reproduce it against your own cartridge:

```sh
git clone --depth 1 https://github.com/pret/pokered /tmp/pokered
PIXELGB_TEST_ROM=~/roms/blue.gb PIXELGB_POKERED_GFX=/tmp/pokered \
  cargo test --release --test species_reference -- --nocapture
```

which checks all 151 species, front and back, not just this one.

**No game art is committed here.** The sprite reaches flash the way the
cartridge already does — an environment variable read at build time — so a fresh
clone builds and this command refuses with the `pixelgb` line to run. 792 bytes
of `.rodata`, memory-mapped out of flash, costing no RAM.

## The colour is ours, and Gen 1 never had any

A DMG draws four greys. This panel has black, white, yellow, red, blue and green
and **no grey at all**. Something has to give, and a still picture is the one
place on this device where it is worth spending the time to get right.

The default look is `red/yellow`: black outline, red shadow, yellow light, white
page. **It is a recolour.** The cartridge has no opinion about Charizard being
orange; the sprite is four greys and always was.

What the recolour does *not* do is invent shading. Those four inks are strictly
ordered by luminance — 0, 57, 225, 255 on Rec. 601 — so a shade the artist drew
darker is still drawn darker. The light-to-dark structure is the cartridge's;
only the hue is a choice. On a fire lizard it happens to land well, and the tail
flame comes out yellow without anyone asking it to.

The faithful alternative is one button away. `dither` is an ordered 8x8 threshold, so the four greys come back as four
apparent greys on a panel that cannot make one. Each Game Boy pixel is 36 panel
pixels at this magnification, which is enough room for the texture to read as
tone rather than as pattern.

Look at both before spending sixteen seconds of panel on either. The converter
takes a third argument and renders every look through the firmware's own
`sprite::draw`, decoded the way the panel decodes its framebuffer — what the
glass is sent, as a PPM:

```sh
rustc --edition 2021 -O -o /tmp/sprite_host platform/m5paper/src/sprite_host.rs
TERMINALGB_SPRITE_NAME=CHARIZARD TERMINALGB_SPRITE_NUMBER='#006' \
  /tmp/sprite_host <that PNG> /tmp/sprite.bin /tmp/preview
```

**No rendered sprite is committed here either.** A whole-number magnification is
reversible — downsample it and the cartridge's own pixels come back — so these
are game data in the way `docs/link-trade/`'s screenshots of the terminal
frontend are not. `docs/upscale-samples/` draws the same line and stays
MIT-ROM-only for the same reason.

## Magnified six times, because six is a whole number

56 becomes 336 on a 600x400 logical screen, with the caption column beside it.
Seven would fit the height and leave four pixels of margin; six leaves
thirty-two and room to breathe.

The scale is a whole number because a Game Boy draws its shading as a one-pixel
checkerboard, and resampling that at a fractional ratio is moiré — the same
finding `gbscreen::SCALE` records for the emulated picture and `chainrender`'s
camera rule 8 records for the swarm films. `sprite::best_scale` picks the
largest whole number that fits, so a 40x40 species gets eight and a 48x48 gets
seven.

## Fourteen point nine seconds, on purpose

The emulator repaints at PLL `0x07`, which nearly halves a refresh. This does
not: it uses the vendor's own `0x08`.

`docs/m5paper-refresh.md` measured the trade. The faster waveform is faster
because the pigments get less time to move, and what that costs — if it costs
anything — is ink separation. Gameplay wants the seconds back. A picture that
will sit there until someone changes it wants the colour.

Measured on the device: **14.930 s**, which is the documented 15.954 s at `0x08`
less the ~1 s that command `0x17` saves by letting the controller sequence its
own power-up.

## What is checked, and what needs eyes

`tools/m5paper-build.sh test` runs 67 host checks with no device in the room.
The ones that matter here: the magnification is a whole number and the picture
fits; shade 0 reaches the paper as ink and shade 3 as page in every look, so a
reversed index cannot flood the page; the caption never overlaps the picture and
every one of its characters has a glyph; the dither's four shades cover four
distinct fractions.

The device reports what it parsed — `CHARIZARD #006 — 56x56, four shades` — and
the 792 bytes it read are byte-identical to the blob those previews were
rendered from, and appear verbatim in the flashed binary.

**Nobody on this end has seen the glass.** The panel has no MISO, so nothing can
be read back from the controller, and that limitation is this whole port's
standing condition rather than anything new here. Everything up to the ink is
verified; whether the ink separates at `0x08` is a person in the room looking at
it.

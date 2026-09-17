# The Pokédex, held on E Ink

Six inks, 400x600, and a picture that costs nothing to keep. Pull the USB lead
out and it is still there — press a button and it is a different picture.

```sh
pixelgb species --rom ~/roms/blue.gb --out /tmp/species
TERMINALGB_SPECIES_DIR=/tmp/species/pokemon-blue/sprites/four-shade/1x \
  tools/m5paper-build.sh pokedex
```

**C** advances to the next Pokédex entry and repaints. **A** goes back to the
previous one. **B** cycles the colour look. Nothing else on the device does
anything, which is the point — `tools/m5paper-build.sh flash` still puts the
running emulator back.

## The pixels are the cartridge's

Every front sprite is run-length-and-delta compressed data somewhere in a
Pokémon Blue image; `pixelgb species`, a sibling project, decompresses all 151
of them in one pass. `src/pokedex_host.rs` runs `src/sprite_host.rs`'s own PNG
decode and shade quantiser once per species and writes the results
concatenated, with a small directory in front naming each one by its own
Pokédex number and the name `pixelgb` gave its file.

That directory is the only thing this module adds. No pixel is touched,
invented or resampled between the PNG `pixelgb` wrote and the byte that reaches
the panel.

The provenance check is the same one the single-species version used, and it
already covered the whole set:

```sh
git clone --depth 1 https://github.com/pret/pokered /tmp/pokered
PIXELGB_TEST_ROM=~/roms/blue.gb PIXELGB_POKERED_GFX=/tmp/pokered \
  cargo test --release --test species_reference -- --nocapture
```

This checks all 151 species, front and back, against `pret/pokered`'s own
committed source art — the artist's original files, which the compressed data
in the cartridge was compiled *from*. Charizard is the one this port measured
by hand first: 0 of 3,136 pixels differ, shade histogram 1,437 / 603 / 567 /
529 identical on both sides, lightest first. Nothing here was drawn, traced or
fetched from the internet.

**No game art is committed here.** The archive reaches flash the way the
cartridge already does — an environment variable read at build time — so a
fresh clone builds and `tools/m5paper-build.sh pokedex` refuses with the
`pixelgb` command to run rather than shipping a placeholder. The whole archive
for 151 species is on the order of 100 KB of `.rodata`, memory-mapped out of
flash, costing no RAM whatever its size — the same trick the single 792-byte
Charizard blob used, and the same trick `TERMINALGB_ROM` uses for the
cartridge itself.

## The archive format

One file, one `include_bytes!`:

```text
[0..4)   magic "GBPX"
[4..6)   count: u16 LE
directory, `count` entries of 22 bytes each:
  number (u16), name (12 bytes, space-padded), offset (u32), length (u32)
payload: `count` sprite blobs, concatenated — each one exactly the format
  the single-species build already wrote and `sprite::Sprite::parse` already
  read: magic "GBSP", width (u16), height (u16), packed 2bpp pixels.
```

`src/pokedex.rs` validates every entry's sprite at boot, once, so a button
press can never fail on a device with no screen of its own to explain why —
the same refuse-loudly discipline the single-species firmware used for its one
picture.

## The colour is ours, and Gen 1 never had any

A DMG draws four greys. This panel has black, white, yellow, red, blue and
green and **no grey at all**. Something has to give, and a still picture is
the one place on this device where it is worth spending the time to get right.

The default look is `red/yellow`: black outline, red shadow, yellow light,
white page. **It is a recolour.** The cartridge has no opinion about what
colour any of these species are; every sprite is four greys and always was.

What the recolour does *not* do is invent shading. Those four inks are
strictly ordered by luminance — 0, 57, 225, 255 on Rec. 601 — so a shade the
artist drew darker is still drawn darker. The light-to-dark structure is the
cartridge's; only the hue is a choice.

The faithful alternative is one button away. `dither` is an ordered 8x8
threshold, so the four greys come back as four apparent greys on a panel that
cannot make one. At the smallest species' magnification (40x40, ×8) a Game Boy
pixel is 64 panel pixels; at the largest (56x56, ×6) it is 36 — either way
there is enough room for the texture to read as tone rather than as pattern.

## Magnified by a whole number, because a whole number is the only safe one

A Game Boy front sprite is one of three sizes in this cartridge — 40x40,
48x48 or 56x56 — and `sprite::best_scale` picks the largest whole-number
magnification that fits the panel's picture column for whichever one is on
screen: ×8 for the smallest, ×6 for the largest.

The scale is a whole number because a Game Boy draws its shading as a
one-pixel checkerboard, and resampling that at a fractional ratio is moiré —
the same finding `gbscreen::SCALE` records for the emulated picture and
`chainrender`'s camera rule 8 records for the swarm films.

## Fourteen point nine seconds, on purpose, every time

The emulator repaints at PLL `0x07`, which nearly halves a refresh. This does
not: it uses the vendor's own `0x08`, on every entry, whatever its size —
resolution changes the pixels drawn, not the waveform that draws them.

`docs/m5paper-refresh.md` measured the trade. The faster waveform is faster
because the pigments get less time to move, and what that costs — if it costs
anything — is ink separation. Gameplay wants the seconds back. A picture that
will sit there until someone presses a button wants the colour.

Measured on the device for Charizard: **14.930-14.942 s** across two separate
flashes of this firmware, which is the documented 15.954 s at `0x08` less the
~1 s command `0x17` saves by letting the controller sequence its own
power-up. Every other species pays the same waveform, because nothing about
the archive's per-entry size changes `TRUE_COLOUR_PLL`.

## What is checked, and what needs eyes

`tools/m5paper-build.sh test` runs the host checks with no device in the
room, `src/pokedex.rs`'s own archive-parsing tests among them: every entry's
number, name and picture size round-trip; an archive with no entries, a
truncated directory, or an offset that overruns the payload are all refused
rather than read out of bounds; a name longer than its field is refused at
*build* time by `src/pokedex_host.rs`, not truncated silently; and a lap of
151 presses of next undoes exactly with 151 presses of previous.

`src/sprite.rs`'s own tests — inherited unchanged, since this module never
touches a pixel — still check that the magnification is a whole number and
the picture fits, that shade 0 reaches the paper as ink and shade 3 as page in
every look, and that the caption never overlaps the picture.

The device reports what it parsed — `N species, four shades each` at boot,
then `NAME #NNN, look X, PLL 0x08, T s` after every repaint, and `i/N` beside
it so the position in the Pokédex is on the serial log even though it cannot
be on the paper.

**Nobody on this end has seen the glass.** The panel has no MISO, so nothing
can be read back from the controller, and that limitation is this whole
port's standing condition rather than anything new here. Everything up to the
ink is verified; whether the ink separates at `0x08`, for any of the 151, is a
person in the room looking at it.

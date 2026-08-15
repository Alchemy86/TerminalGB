# Authoring a Gen 1 area

[← Gen 1 area](README.md)

**How a Pokémon Red/Blue room is put together, what the engine does to load one, and the
recipe for building a new one in memory and walking into it through an ordinary door.**

Everything on this page is for **English Pokémon Red and Blue**, which share their WRAM
layout exactly. Every address carries its `pret/pokered` symbol and how it was
established — see the three grades in the [area standards](README.md#standards-for-this-area)
— plus a fourth used here:

* **symbol file** — `pokered`/`pokeblue` built at RGBDS 1.0.3, both ROMs matching
  `roms.sha1`, so a label's address is exact for the retail cartridge.
* **walked** — added up through `ram/wram.asm` between two symbol-file addresses.
* **cartridge** — read out of a run of machine code in the ROM image.
* **measured** — observed happening, on the retail cartridge, by
  [`examples/warproom_probe.rs`](../../examples/warproom_probe.rs). Where a claim is
  measured, the number it produced is printed beside it.

Anything not in one of those four grades says **inferred** and says why nobody has checked it.

---

## 0. The short version

A Gen 1 area is **six independent things**, and only two of them are geometry:

| | What it is | Where it lives while you are standing in it |
|---|---|---|
| The **block map** | one byte per 4×4-tile block, row-major, no header | WRAM, via `wCurMapDataPtr` |
| The **header** | tileset, height, width, three pointers, connection bits | WRAM `wCurMapHeader` |
| The **tileset** | a block table, a collision list, 96 tiles of art | WRAM pointers + VRAM |
| The **warps** | `(Y, X, warp id, map id)` × N | WRAM `wWarpEntries` |
| The **signs** | `(Y, X)` and a text id, × N | WRAM `wSignCoords` / `wSignTextIDs` |
| The **objects** | up to 15 people/items/trainers | WRAM `wSpriteStateData1` / `2` |

All six are **in RAM** while the map is live. The ROM tables are only the *source* the
engine copies from at load time, and the copy is skippable. That is the whole opportunity,
and section 3 is the recipe.

**This page is the *destination* half; [`docs/worldlobby.md`](../worldlobby.md) is the
*occupancy* half.** The World Trade Centre enlarges the room the player is already standing
in — it keeps `wCurMap` at `TRADE_CENTER`, preserves the vanilla corner byte for byte so the
consoles still work, and fills the rest with a hall — and then puts other people in it. This
page answers the question that leaves open: whether an authored room has to *be* the Cable
Club. It does not. Both work by writing `wOverworldMap`'s source and letting the engine
render; the difference is only whether you also replace the header.

**Measured, end to end:** a 12 × 10-block room (24 × 20 walk tiles — the visible area is 10 × 9, so about two and a half screens each way)
built in `wTileMapBackup2`, entered by walking down the bedroom stairs of a brand-new game
with the cartridge's own unedited warp, walked corner to corner, painted entirely by the
engine (**360 of 360 on-screen tiles** from our three blocks, zero foreign), given its own
door back out, and left again — after which the cartridge's own 4 × 4 bedroom came back from
ROM on its own. No fault, nothing in the save file.

---

## 1. What an area actually is

### 1.1 Blocks, tiles and the two coordinate systems

Three units, and confusing two of them is the commonest way to get a room wrong.

```
1 block      = 4 × 4 screen tiles = 2 × 2 walk tiles
1 walk tile  = 2 × 2 screen tiles = 16 × 16 pixels     <- wYCoord / wXCoord count these
1 screen tile= 8 × 8 pixels                            <- wTileMap holds these
```

`BLOCK_WIDTH`/`BLOCK_HEIGHT` are 4 (`constants/gfx_constants.asm`, symbol file).
`wCurMapWidth`/`wCurMapHeight` are in **blocks**; `wCurrentMapWidth2`/`wCurrentMapHeight2`
are the same map in **walk tiles**, and the engine keeps both. Section 5 explains why
setting only one is a silent trap.

### 1.2 The block map

A `maps/*.blk` file is **exactly** the bytes the engine copies: `height` rows of `width`
bytes, each byte a block id, no header, no padding, no terminator. Red's House 2F is sixteen
bytes. Nothing about the format is compressed or indirect, which is why a block map can be
hand-written into WRAM and pointed at.

### 1.3 The header, and what is in ROM versus RAM

`map_header` (`macros/scripts/maps.asm`, symbol file) emits eleven bytes plus connections:

```
db  tileset
db  height, width          ; blocks
dw  <blocks>               ; -> the .blk data
dw  <text pointers>
dw  <script>
db  connection bitfield    ; EAST 1, WEST 2, SOUTH 4, NORTH 8
    <0-4 connection structs, 11 bytes each>
dw  <object data>          ; end_map_header
```

The first nine bytes land verbatim in `wCurMapHeader`:

| Symbol | Address | Size | Grade |
|---|---|---|---|
| `wCurMapHeader` = `wCurMapTileset` | `$D367` | 1 | symbol file |
| `wCurMapHeight` (blocks) | `$D368` | 1 | symbol file |
| `wCurMapWidth` (blocks) | `$D369` | 1 | symbol file |
| `wCurMapDataPtr` | `$D36A` | 2 | symbol file |
| `wCurMapTextPtr` | `$D36C` | 2 | symbol file |
| `wCurMapScriptPtr` | `$D36E` | 2 | symbol file |
| `wCurMapConnections` | `$D370` | 1 | symbol file |
| `wCurMapHeaderEnd` | `$D371` | — | symbol file |

`wCurMapHeaderEnd - wCurMapHeader` is literally the byte count `LoadMapHeader`'s copy loop
uses, so the layout is not a guess: it is the loop bound.

### 1.4 The object data — border block, warps, signs, objects

`end_map_header`'s pointer leads to a second ROM structure, and this one is a stream, not a
struct. `LoadMapHeader` walks it in this order:

```
db  border block                      -> wMapBackgroundTile   $D3AD
db  number of warps                   -> wNumberOfWarps       $D3AE
    4 bytes each: Y, X, warpID-1, map -> wWarpEntries         $D3AF   (32 max)
db  number of signs                   -> wNumSigns            $D4B0
    3 bytes each: Y, X, text id       -> wSignCoords          $D4B1   (16 max)
                                       + wSignTextIDs         $D4D1
                                         (the stream interleaves them; the
                                          engine de-interleaves into two arrays)
db  number of objects                 -> wNumSprites          $D4E1
    6-8 bytes each                    -> wSpriteStateData1/2 + wMapSpriteData/ExtraData
    <destination-warp table>          -> read later, by warp id
```

All symbol file. The maxima are `MAX_WARP_EVENTS` 32, `MAX_BG_EVENTS` 16 and
`MAX_OBJECT_EVENTS` 16 (`constants/map_data_constants.asm`), and `def_warps_to` asserts all
three at assembly time — so they are the game's own ceilings, not ours.

**`warp_event x, y, map, id` emits `db y, x, id-1, map`.** The macro takes X first and the
data stores Y first, and the warp id is stored one lower than it is written. Both catch
people out.

An **object event** is `db picture, y+4, x+4, movement1, movement2, textid` — note the
**+4 on both coordinates**, which is why `SPRITESTATEDATA2_MAPY`/`MAPX` read four higher
than `wYCoord`/`wXCoord`. A trainer or item adds one or two bytes and sets `BIT_TRAINER`
(6) or `BIT_ITEM` (7) in the text-id byte.

The trailing **destination-warp table** is what places the player when somebody warps *in*.
`def_warps_to` emits one four-byte record per warp:

```
dw  wOverworldMap + 7 + W + (W + 6) * (y >> 1) + (x >> 1)
db  y, x
```

(`event_displacement`, `macros/coords.asm`, symbol file). Those four bytes are copied
straight over `wCurrentTileBlockMapViewPointer` (2), `wYCoord` and `wXCoord` — which are
consecutive in WRAM, `$D35F`, `$D361`, `$D362` — by `LoadDestinationWarpPosition`. So the
view pointer is not derived at run time; it is *tabulated*, and if you author a map you have
to compute it yourself. Section 3.4 does.

The formula places the view's upper-left corner **two blocks above and two blocks left** of
the player's block, which is what puts the player at screen tile (8,8). Derivation: map
block `(bx, by)` sits at `wOverworldMap + (3 + by)·(W+6) + 3 + bx`, and equating that with
the formula gives `bx − 2`, `by − 2`.

### 1.5 The tileset

Four WRAM values, loaded by `LoadTilesetHeader` from the twelve-byte `Tilesets` table:

| Symbol | Address | What | Grade |
|---|---|---|---|
| `wTilesetBank` | `$D52B` | ROM bank for the art | symbol file |
| `wTilesetBlocksPtr` | `$D52C` | 16 bytes per block, 4×4 tile ids, row-major | symbol file |
| `wTilesetGfxPtr` | `$D52E` | 96 tiles of 2bpp art | symbol file |
| `wTilesetCollisionPtr` | `$D530` | `$FF`-terminated list of **passable** tile ids | symbol file |

**`wTilesetBlocksPtr` and `wTilesetCollisionPtr` are dereferenced bank-agnostically** —
`DrawTileBlock` and `CheckTilePassable` (`home/overworld.asm`) both use a plain
`ld a, [de]` / `ld a, [hli]` with no bank switch — so both may point at WRAM. Custom blocks
and custom walkability are reachable, not just a rearrangement of Nintendo's. Precedent that
this is an intended-looking thing to do: `CableClub_Run` (`engine/link/cable_club.asm`)
rewrites three of these four pointers at run time itself.

### 1.6 Collision: the rule, and the off-by-one that will bite you

Collision does **not** read the block map. `_GetTileAndCoordsInFrontOfPlayer`
(`engine/overworld/player_state.asm`) probes `wTileMap` — the 20 × 18 *screen* buffer — at
`lda_coord 8, 11` (down), `8, 7` (up), `6, 9` (left) and `10, 9` (right), and
`CheckTilePassable` tests that one byte against the `wTilesetCollisionPtr` list.

The player's own walk tile always occupies screen tiles (8..9, 8..9). So each probe is the
**bottom-left** screen tile of the target walk tile — *not* the top-left. Within a block's
16 row-major tiles that is:

```
block tile indices        walk tile   probe index
 0  1  2  3               (0,0)            4
 4  5  6  7               (1,0)            6
 8  9 10 11               (0,1)           12
12 13 14 15               (1,1)           14
```

**Cross-checked** against the nine distinct blocks Red's bedroom is built from and the
routes the probe's walker actually took through it: the top-left rule and the bottom-left
rule agree for uniform blocks and disagree for every mixed one, and only the bottom-left
rule predicts which squares the player could stand on. The staircase is the worked example —
block `$08`'s walk tile (1,1) is `$0A` at index 10 and `$1A` at index 14, the collision list
contains `$1A` and not `$0A`, and **measured**, the player walks onto it. Get this wrong and
your "wall" is walkable half the time.

`IsPlayerStandingOnDoorTileOrWarpTile` uses the same convention for the tile the player is
*standing on*: `lda_coord 8, 9`.

**Two crews reached the same anchor by different routes on the same day**, which is the
strongest form this repository asks for. The [lobby](../worldlobby.md) crew's
`screen::calibrate` brute-forces the candidate rows by reproducing all 360 screen cells from
the cartridge's own block map: **row 8 scores 360/360 and row 9 scores 133/360** for the
walk tile's top-left, so the top-left is (8,8) and the collision probe at (8,9) is the row
below it. This page derives the same thing from `LoadCurrentMapView`'s sub-block adjustment.
Derivation and brute force agree.

### 1.7 Objects

Sixteen 16-byte structs at `wSpriteStateData1` `$C100` and sixteen more at
`wSpriteStateData2` `$C200` (`NUM_SPRITESTATEDATA_STRUCTS` = 16,
`constants/map_object_constants.asm`). **Slot 0 is the player**, so a map has fifteen
object slots — one fewer than `MAX_OBJECT_EVENTS`. Field offsets worth knowing:
`PICTUREID` +0, `IMAGEINDEX` +2, `YPIXELS` +4, `XPIXELS` +6, `FACINGDIRECTION` +9 in
`…Data1`; `MAPY` +4, `MAPX` +5 (both *map coordinate + 4*), `IMAGEBASEOFFSET` +$E in
`…Data2`.

Only nine remote avatars can be drawn at once before OAM overflows — measured by an earlier
scout, and out of scope here.

---

## 2. How the engine loads an area

`WarpFound1` / `WarpFound2` (`home/overworld.asm`) set `wCurMap` — and, on the two warp
paths, `wDestinationWarpID` — and jump to `EnterMap`, whose first act is `LoadMapData`:

```
LoadMapData
├── DisableLCD                     ; spins until VBlank  <-- the window, section 3.3
├── LoadTextBoxTilePatterns
├── LoadMapHeader
│   ├── MarkTownVisitedAndLoadToggleableObjects
│   ├── SwitchToMapRomBank(wCurMap)
│   ├── **if BIT_NO_PREVIOUS_MAP was set on wCurMapTileset: clear it and RETURN**
│   ├── copy 9 header bytes from MapHeaderPointers[wCurMap]
│   ├── copy 0-4 connection structs (11 bytes each)
│   ├── walk the object data: border block, warps, signs, objects
│   ├── predef LoadTilesetHeader   ; 4 tileset pointers; then, if the tileset changed,
│   │                              ; LoadDestinationWarpPosition -> view ptr + wYCoord/wXCoord
│   ├── callfar LoadWildData
│   ├── wCurrentMapHeight2 = 2 × wCurMapHeight, wCurrentMapWidth2 = 2 × wCurMapWidth
│   └── map music
├── InitMapSprites                 ; sprite tile patterns
├── LoadTileBlockMap               ; fill 1300 bytes with wMapBackgroundTile,
│                                  ; then copy H rows of W bytes from **wCurMapDataPtr**
│                                  ; into a 3-block border; then connection strips
├── LoadTilesetTilePatternData     ; 96 tiles from wTilesetGfxPtr:wTilesetBank -> $9000
├── LoadCurrentMapView             ; wOverworldMap -> wSurroundingTiles -> wTileMap
├── copy wTileMap -> vBGMap0
└── EnableLCD
```

Two properties of that chain are the whole of this page.

**(a) `LoadTileBlockMap` reads the block map with a plain `ld a, [de]`.** No bank switch, no
range check. `wCurMapDataPtr` may point at WRAM and the engine will build a room out of it.
It may **not** point into a different ROM bank, because `LoadMapHeader` has already
switched to `wCurMap`'s bank — which closes off the cheapest-looking trick, "borrow another
room's geometry".

**(b) `LoadMapHeader` can be told to skip itself.** The five instructions at its top:

```asm
ld a, [wCurMapTileset]
ld b, a
res BIT_NO_PREVIOUS_MAP, a
ld [wCurMapTileset], a
ldh [hPreviousTileset], a
bit BIT_NO_PREVIOUS_MAP, b
ret nz
```

`BIT_NO_PREVIOUS_MAP` is **bit 7** (`constants/ram_constants.asm`, symbol file). This is the
latch [`memory-map.md` §4](memory-map.md#4-the-map-the-player-and-the-world) records as a
*trap*: `LoadSAV` sets it, so a continued game trusts the header held in `sMainData`, and
editing `wCurMap` without the header makes the game read the new map through the old map's
pointer and die within a second.

Set it deliberately and it is not a trap, it is the authoring hook. The header load is
skipped, `LoadMapData` carries straight on, and `LoadTileBlockMap` builds the room from
whatever you put in `wCurMapDataPtr`. **It clears itself**, so the next warp loads normally.

---

## 3. The recipe

Run it: `cargo run --release --features image --example warproom_probe -- --rom <blue.gb>
[--shots <dir>] [--no-latch]`. It starts a **new game** — no save file needed — walks to the
bedroom stairs, and goes down them into a room that exists only in WRAM.

### 3.1 What must already be true

* **`wCurMap` must be a real map id.** `SwitchToMapRomBank` and
  `MarkTownVisitedAndLoadToggleableObjects` both index ROM tables by it *before* the latch
  is tested. The probe leaves it at `REDS_HOUSE_1F` — the stairs' own destination — so
  every ROM table lookup is the genuine, intended one and only the header is ours. Pick an
  id below `FIRST_ROUTE_MAP` and you also mark a town visited for Fly.
* **A scratch buffer for the block map, below `wMainDataStart` `$D2F7` and outside
  `wOverworldMap` `$C6E8..$CBFC`.** `LoadTileBlockMap` fills the whole 1300-byte overworld
  buffer with `wMapBackgroundTile` before copying, so a source inside it is destroyed
  mid-copy. `wTileMapBackup2` `$CD81` is 360 bytes and the largest clean gap; it is only
  written when a menu takes a screen snapshot.
* **A tileset already loaded.** Skipping the header also skips `LoadTilesetHeader`, so the
  four tileset pointers keep the *departing* room's values. That is usually what you want:
  the blocks, the collision list and the art are already there.

### 3.2 Choosing blocks without hard-coding any

Derive them from the cartridge rather than tabulating them, and the recipe works for any
tileset. The probe reads `wTilesetCollisionPtr`'s `$FF`-terminated list and the 16-byte
block definitions through `wTilesetBank`, then classifies each block by testing indices
4, 6, 12, 14 (§1.6) against the list:

* **floor** — all four walk tiles passable, preferring a block made of one repeated tile;
* **wall** — none passable, same preference;
* **the door** — lifted straight out of the departing room's live block map at the block
  containing the warp you are leaving through.

That last one matters more than it looks; see §3.6.

Measured on `RedsHouse2`: 9 passable tile ids (`01 02 03 11 12 13 14 1c 1a`), floor `$0F`,
wall `$0A`, door `$08`. `$0A` is also the border block Red's House 2F declares in ROM, which
is an independent check that the classifier agrees with the cartridge.

### 3.3 Where the writes have to land — and why *not* before the step

The obvious thing is to install everything and then walk through the door. **It does not
work, and the reason is a good one:** a Gen 1 warp is decided from the *screen*.

`CheckWarpsNoCollision` matches `wYCoord`/`wXCoord` against `wWarpEntries` and then calls
`IsPlayerStandingOnDoorTileOrWarpTile`, which reads `lda_coord 8, 9` — the player's own tile
in `wTileMap` — and looks it up in `WarpTileIDPointers[wCurMapTileset]`. Two of the bytes
you want to write feed that decision while the player is still in the departing room:

* **`wCurMapWidth`** is the stride `RedrawRowOrColumn` uses to repaint the column that
  scrolls in as the player steps, so a changed width paints the departing room out of the
  wrong bytes of `wOverworldMap`; and
* **`wCurMapTileset`** — the byte the latch lives in — is the *index* into
  `WarpTileIDPointers` and `DoorTileIDPointers`. With bit 7 set that index is off the end of
  both tables.

**Measured:** installing first left the player standing on the bedroom staircase, in the
bedroom, unable to leave.

The window the engine leaves is `LoadMapData`'s own first instruction, `call DisableLCD`,
which spins until VBlank. So `wCurMap` has already changed a whole frame boundary before
`LoadMapHeader` runs. **Measured:** stepping into the stairs, `wCurMap` read `$25` on frame
15 of the step while `wOverworldMap[0]` still held the *bedroom's* border block — the block
map had not been touched. Everything in §3.4 was written there, and all fifteen bytes
survived.

A host driving the emulator frame by frame gets this for free: poll `wCurMap` after each
frame, and write on the frame it changes.

### 3.4 The writes

With the latch, in the window, all of these are kept — **measured, 15 of 15**:

| Write | Address | Why |
|---|---|---|
| `wCurMapHeight` = H blocks | `$D368` | `LoadTileBlockMap`'s row count |
| `wCurMapWidth` = W blocks | `$D369` | row length **and** the stride `W + 6` |
| `wCurMapDataPtr` = scratch | `$D36A` | the room |
| `wCurMapConnections` = 0 | `$D370` | no connection strips to load |
| `wMapBackgroundTile` = wall | `$D3AD` | fills the 3-block border and everything past it |
| `wNumSigns` = 0 | `$D4B0` | the departing room's signs are still listed otherwise |
| `wNumSprites` = 0 | `$D4E1` | likewise its people |
| `wCurrentMapHeight2` = 2H | `$D524` | **walk tiles** — the edge check reads this, §5 |
| `wCurrentMapWidth2` = 2W | `$D525` | walk tiles |
| `wCurrentTileBlockMapViewPointer` | `$D35F` | `wOverworldMap + 7 + W + (W+6)·(y>>1) + (x>>1)` |
| `wYBlockCoord`, `wXBlockCoord` | `$D363`, `$D364` | `y & 1`, `x & 1` |
| `wCurMapTileset` \| `$80` | `$D367` | the latch, written last |

Plus the block map itself into the scratch buffer.

**Two header bytes are deliberately left alone and you should know it.**
`wCurMapTextPtr` `$D36C` and `wCurMapScriptPtr` `$D36E` keep the *departing* room's values,
so the room runs the previous map's script every frame and any sign in it would read the
previous map's text. Red's bedroom script is a one-shot no-op
(`RedsHouse2FDefaultScript`, `scripts/RedsHouse2F.asm`, sets the player's facing once and
switches itself to `SCRIPT_REDSHOUSE2F_NOOP`), which is why the probe can ignore it. A room
authored behind a door whose room has a *live* script has to point these somewhere, and
nobody has tried that yet.

**The player's arrival coordinates are whatever they were when the warp fired**, because
`LoadDestinationWarpPosition` is one of the things the skipped header load would have done.
Design the room around that tile rather than trying to move the player; a teleport with no
`LoadCurrentMapView` behind it leaves the screen stale until the next step.

### 3.5 What the engine does for you afterwards

Everything. **Measured:** the probe paints `wTileMap` **never** — not once — and after
walking 21 tiles east and 16 south, all **360 of 360** on-screen tiles come from the three
blocks it placed. Walking, the four-frame walk cycle, collision, the collision sound, the
camera and the scroll are the cartridge's, operating on data we supplied.

This is also why authoring `wOverworldMap`'s *source* beats painting `wTileMap` directly:
painting the screen races `RedrawRowOrColumn` and leaves diagonal streaks, whereas the block
map has exactly one writer.

### 3.6 Giving the room a door

A warp needs **two** things, and this is the finding that cost the most here:

1. an entry in `wWarpEntries` whose Y and X match the player's; **and**
2. a tile the *tileset* calls a warp tile.

Without (2), `IsPlayerStandingOnDoorTileOrWarpTile` returns no carry and the only remaining
route is `ExtraWarpCheck` → `IsWarpTileInFrontOfPlayer`, which additionally requires a
direction to be **held** at the instant of the check. **Measured:** a warp entry placed on a
plain floor block does not fire when the player simply steps onto it.

The cheap fix is the one in §3.2 — lift the departing room's own staircase block out of its
live block map and place *that*. It already carries the stair tile (`$1A` for
`RedsHouse2`, from `data/tilesets/warp_tile_ids.asm`), it is already passable, and its top
half is solid, which is exactly what an arrival tile on the room's top edge wants.

**Measured, the whole round trip:** with the door block in place and one authored entry
`(Y=1, X=7) → REDS_HOUSE_2F warp 0`, stepping back onto it returned the player to the
bedroom — and because the latch had cleared itself, that return ran a *normal*
`LoadMapHeader` and the cartridge's own 4 × 4 bedroom came back from ROM. **Nothing
authored outlives the visit unless you re-install it.**

### 3.7 Custom art

`LoadTilesetTilePatternData` runs **after** `LoadMapHeader` and is not skipped by the latch.
It copies `MAP_TILESET_SIZE` = 96 tiles (`$9000-$95FF`) from `wTilesetGfxPtr` in
`wTilesetBank`, unconditionally. **Measured:** a signature written into `$9000 + $50·16`
inside the install window read back as the cartridge's own art afterwards — **clobbered**.

Two routes, and only one of them is cheap:

* Point `wTilesetGfxPtr`/`wTilesetBank` at a WRAM buffer holding your 1536 bytes, so every
  engine-initiated reload restores *yours*. There is no 1536-byte contiguous gap below
  `wMainDataStart` (the largest is `wTileMapBackup2`'s 360), so this needs the block map to
  live somewhere else again.
* **Re-blit from the host after arrival.** **Measured:** writing an eight-row diagonal weave
  over tile `$01` — the tile every walkable block of this tileset is made of — read back
  exactly, and the floor of the room is then a pattern that exists nowhere in Nintendo's
  cartridge. It costs no emulated cycles at all.

The same reload discipline applies afterwards: `ReloadMapData` (closing a START submenu,
the Pokédex, a PC, using an item), `ReloadTilesetTilePatterns` (after a battle or evolution)
and any further `LoadMapData` all restore the cartridge's art over yours. A host that runs
every frame can diff and re-blit.

**`$9600` onward is the text box** (`LoadTextBoxTilePatterns`), `$8800` is the font and
`$8000` is sprite tiles. Overwrite any of them and dialogue breaks. The 96 tiles at `$9000`
are yours; nothing else is.

### 3.8 The run, as it prints

Trimmed; run it yourself for the rest.

```text
cartridge: POKEMON BLUE (checksum $9D0A)
in the bedroom: map $26 at (3, 6), 4x4 blocks
tileset: bank $19, blocks @ $5270, collision @ $1749
  9 passable tile ids: 01 02 03 11 12 13 14 1c 1a
  64 blocks; floor $0F (all four walk tiles passable), wall $0A (none),
             back-wall $03 (top solid, bottom walkable)
door block: $08, lifted from the bedroom's own block map at block (3, 0)
room: 12x10 blocks = 24x20 walk tiles;
      wOverworldMap footprint (W+6)*(H+6) = 288 of 1300 bytes

stepping into the stairs at (7, 1)...
  warp committed on frame 15: wCurMap $25. wOverworldMap[0] is still $0A
  (the bedroom's border block), so LoadTileBlockMap has not run — installed
  120 block-map bytes and the header here, latch SET.

-- what survived LoadMapData --            (15 rows, all "kept")
  wOverworldMap row 0 (stride 18, $C721): 0a 0a 0a 08 0a 0a 0a 0a 0a 0a 0a 0a
  our block-map row 0:                    0a 0a 0a 08 0a 0a 0a 0a 0a 0a 0a 0a
  => the live block map IS the one we wrote

-- custom tile art --
  signature written to vTileset tile $50 before the load reads back 00 00 24 25
  => CLOBBERED
  re-blitted our own floor pattern over tile $01 after arrival => OURS

-- walking the room --
  reached (21, 17) of a 24x20 walk-tile room; every rest inside the extent,
  fault None
  on-screen buffer after the walk, painted by the ENGINE from our block map:
    360 of 360 tiles come from the three blocks we placed, 0 from anything else

-- leaving through the warp we authored --
  map $26 at (7, 1), 4x4 blocks, tileset $04
  => the return warp ran a NORMAL LoadMapHeader (latch self-cleared)

-- save safety --
  saved header reads map $00, 0x0 blocks, wCurMapDataPtr $0000
  => nothing we authored is in it.
== PASS ==
```

`--shots <dir>` writes five frames: the bedroom, the room, the room with our own floor,
the far corner, and the bedroom again. **They are deliberately not committed** — they are
raw game frames rather than screenshots of our own interface, and the
[licence boundary](README.md#standards-for-this-area) keeps those out of the repository.
Regenerate them in about a minute.

---

## 4. The ceilings

Real numbers, with their sources. A room bigger than the buffer is a crash, not a big room.

| Limit | Value | Where it comes from |
|---|---|---|
| **Block-map buffer** | `(W + 6) · (H + 6) ≤ 1300` blocks | `wOverworldMap` `$C6E8` .. `wOverworldMapEnd` `$CBFC` (symbol file). `LoadTileBlockMap` lays the map out with `MAP_BORDER` = 3 blocks on every side, stride `W + 6`, map row `r` at `(3 + r)·(W+6) + 3` |
| Biggest cartridge map | 10 × 72 blocks = **1248** | `ROUTE_23` and `ROUTE_17`, `constants/map_constants.asm` — i.e. the rule above is the one the game itself obeys, with 52 bytes to spare |
| Largest square room | **30 × 30 blocks** (60 × 60 walk tiles) | `36·36 = 1296 ≤ 1300` |
| Widest hall | 80 × 9 blocks (160 × 18 walk tiles) | `86 · 15 = 1290` |
| **Coordinate range** | 127 blocks / 254 walk tiles per axis | `wYCoord`/`wXCoord` are `u8`, so a walk coordinate cannot exceed 255 |
| **Warps** | 32 | `MAX_WARP_EVENTS`, `constants/map_data_constants.asm`; `wWarpEntries` is `ds MAX_WARP_EVENTS * 4` |
| **Signs** | 16 | `MAX_BG_EVENTS` |
| **Objects declared** | 16 | `MAX_OBJECT_EVENTS` |
| **Objects usable** | **15** | `NUM_SPRITESTATEDATA_STRUCTS` = 16 and slot 0 is the player |
| **Blocks per tileset** | 256 | a block id is one byte; `DrawTileBlock` computes `id × 16` over the full range |
| **Tiles per tileset** | **96**, `$9000-$95FF` | `MAP_TILESET_SIZE` = `$60`, `constants/tileset_constants.asm`; `$9600` is the text box |
| Scratch for a block map | 360 bytes | `wTileMapBackup2` `$CD81`, the largest clean gap below `wMainDataStart` |
| On-screen buffer | 20 × 18 screen tiles | `wTileMap` `$C3A0` |
| Intermediate buffer | 24 × 20 screen tiles | `wSurroundingTiles`, `SCREEN_BLOCK_WIDTH` 6 × `SCREEN_BLOCK_HEIGHT` 5 blocks |

Note the two ceilings interact: 360 bytes of scratch caps a room at 360 blocks, which is
below the 1300-byte footprint limit for anything squarer than about 19 × 19. Move the block
map somewhere larger and the buffer becomes the binding constraint again.

**Measured:** 12 × 10 blocks needs 288 of 1300, and the probe asserts the footprint before
writing a byte.

---

## 5. What must never be touched

1. **Everything outside the map extent must be wall.** There is no boundary test anywhere in
   the collision check — one tile of `wTileMap` is the entire test. An earlier scout painted
   the whole screen buffer as floor, walked north out of the room, and watched `wYCoord`
   underflow through `(234, 17)` and `(255, 255)` with no fault raised. Ring the room, and
   set `wMapBackgroundTile` to the wall block so the 3-block border matches.
2. **`wCurMapWidth`/`wCurMapHeight` are not the only dimensions.** `wCurrentMapHeight2`
   `$D524` and `wCurrentMapWidth2` `$D525` hold the same map in **walk tiles**, and it is
   *those* the edge-of-map connection check reads. Set only the block pair and the engine
   still believes the room is its old size: the player walks past the edge, finds no
   connection, and lands on map `$00` at (0,57) with no fault raised. Measured once, by
   somebody else, and it looked exactly like a corrupt block map.
3. **`wCurrentTileBlockMapViewPointer` must agree with the width and the player's
   coordinates**, by the cartridge's own formula (§1.4). Get it wrong and the first frame
   shows the wrong part of the room.
4. **Do not set the latch, or change `wCurMapWidth`, while the player can still walk in the
   departing room.** §3.3 — both feed the decision that makes a staircase a staircase.
5. **Never write sprite slot 0.** That is the player.
6. **Never write `wShadowOAM` (`$C300-$C39F`) directly.** The game rebuilds it every frame
   from the sprite structs, which are the correct place to write.
7. **The header is saved state.** `wCurMap`, `wCurrentTileBlockMapViewPointer`, `wYCoord`,
   `wXCoord`, `wCurMapHeader`, `wWarpEntries`, `wNumSigns` and the rest all sit at or above
   `wMainDataStart` `$D2F7`, i.e. inside `sMainData`. Nothing reaches the medium until
   `SaveMainData` runs — the player has to walk into a Pokémon Center and choose SAVE — but
   a feature that leaves an authored room installed while a player can reach a save menu is
   one save away from a `.sav` that points at WRAM. Gate saved-region writes on a state where
   saving is impossible, or restore the header before the player can reach a menu.
   **Measured:** after the probe's whole round trip the staged `.sav`'s saved header still
   reads map `$00`, `0 × 0` blocks, `wCurMapDataPtr $0000` — nothing authored is in it.
8. **`wTileMapBackup2` is scratch that something else also uses.** A menu screen-snapshot
   writes 360 bytes over it. Do not open a menu with a room living there.

### 5.1 What the engine clobbers, measured both ways

`warproom_probe --no-latch` writes exactly the same fifteen bytes with bit 7 clear. The
engine's own load of `REDS_HOUSE_1F` then runs, and **10 of the 15 measurably changed**:

```
  wCurMapHeight (blocks)              $D368  $0A -> $04
  wCurMapWidth (blocks)               $D369  $0C -> $04
  wCurMapDataPtr lo                   $D36A  $81 -> $09
  wCurMapDataPtr hi                   $D36B  $CD -> $42
  wNumSigns                           $D4B0  $00 -> $01
  wNumSprites                         $D4E1  $00 -> $01
  wCurrentMapHeight2 (walk tiles)     $D524  $14 -> $08
  wCurrentMapWidth2 (walk tiles)      $D525  $18 -> $08
  wCurrentTileBlockMapViewPointer lo  $D35F  $FE -> $F6
  wCurMapTileset (+ latch)            $D367  $04 -> $01
```

and the live block map came out as the cartridge's Red's House 1F, not ours. The remaining
five (`wCurMapConnections`, `wMapBackgroundTile`, `wCurrentTileBlockMapViewPointer` hi,
`wYBlockCoord`, `wXBlockCoord`) are **also rewritten** — they are inside the fixed-header
copy loop, the object-data walk, or `LoadDestinationWarpPosition` — they simply happen to
land on the same value for this pair of rooms. *Inferred from the source for those five;
the ten above are measured.*

That is the case for the latch stated as a measurement rather than an argument: the header
is re-read from `MapHeaderPointers` before `LoadTileBlockMap` ever looks at
`wCurMapDataPtr`, so without the latch there is no ordering of host writes that survives.

---

## 6. Conformance record

Per the standing convention in [`AGENTS.md`](../../AGENTS.md), and like the terminal-render
entries, this is a **capability** finding rather than an emulation bug, so point 3 cites the
disassembly rather than Pan Docs.

*Symptom* — none: nothing here is a defect in the emulator. What it documents is a
capability nobody had written down, and two ways of reaching for it that fail silently
(installing before the step; a warp entry on a tile the tileset does not call a warp tile).
Both fail by *doing nothing* — the player stands on a staircase that is no longer a
staircase — which is the hardest kind of failure to diagnose from the outside.

*Caught by* — **no test ROM, and none can**: every claim here is about a commercial
cartridge's data structures, which no conformance suite models. The check is
[`examples/warproom_probe.rs`](../../examples/warproom_probe.rs), which runs against the
user's own cartridge and prints its evidence.

*Contract* — `pret/pokered`: `LoadMapHeader`, `LoadTileBlockMap`, `LoadMapData`,
`CheckWarpsNoCollision`, `IsPlayerStandingOnDoorTileOrWarpTile` and `ExtraWarpCheck` in
`home/overworld.asm` and `engine/overworld/player_state.asm`;
`BIT_NO_PREVIOUS_MAP` in `constants/ram_constants.asm`; the maxima in
`constants/map_data_constants.asm`; `event_displacement` in `macros/coords.asm`.

*Conformance candidate* — none for an emulator. The candidate this suggests for **our own**
tooling is different and worth stating: **a host that writes emulated memory should be able
to say which of its writes survived.** The probe's "wrote / now / kept-or-clobbered" table is
the whole reason the latch is a measurement here instead of a story, and it took ten lines.

---

## 7. Generation 2 — the work, not a guess

**There is no `docs/gen2/` and no Gen 2 addresses appear on this page, deliberately.** Gen 2
is not Gen 1 with different numbers; the map system was rebuilt. Publishing a table of
plausible-looking addresses would be worse than publishing nothing, because the failure mode
is a plausible-looking room that corrupts a save.

What is **established** about Gen 2 in this repository already, and can be relied on:

* **The disassembly is [`pret/pokecrystal`](https://github.com/pret/pokecrystal)**, and for
  Gold specifically [`pret/pokegold`](https://github.com/pret/pokegold), which **builds
  byte-identical to the retail cartridge** with RGBDS 1.0.3 (SHA-1 `d8b8a360…`, matching its
  own `roms.sha1`). So a Gen 2 symbol file can be made exact the same way this page's was.
  Derive constants with `rgbasm -Q8 -P includes.asm -D _GOLD` and `PRINTLN "{d:SYMBOL}"`
  rather than counting by hand.
* **A Gen 2 continue does not load map objects.** `MapSetupScript_Continue`'s second command
  is `LoadMapAttributes_SkipObjects`: it assumes you are on the map you saved on. Editing
  `wMapGroup` in a save lands you on the right map with the previous map's NPCs and a stale
  camera. Override the warp destination at run time instead — `wNextWarp` / `wNextMapGroup`
  / `wNextMapNumber`, written *between instructions*, because the game reads them back a few
  instructions after writing them and a per-frame poke loses the race.
* **Gen 2's save is three separate WRAM runs into three separate SRAM runs**
  (`wPlayerData` → `sPlayerData`, `wCurMapData` → `sCurMapData`, `wPokemonData` →
  `sPokemonData`), so a save offset is *not* one base plus an offset, and the 16-bit
  checksum over `sGameData` is verified, not tolerated.
* `wTilemap` is `$C3A0` and a tile id is still a character code, so the screen still reads
  back as text — but the **menu cursor is tile `$ED`** and `wMenuCursorY` reads 2 whatever
  the cursor is doing.

All four are from this repository's Time Capsule work; see
[`docs/link-cable.md`](../link-cable.md) §8.

### What a Gen 2 equivalent of this page needs

1. **A map is addressed by `(group, number)`, not one id.** Every table lookup, every warp
   entry and every "am I on the right map?" test gains a second byte. That alone invalidates
   a one-for-one translation of §1.
2. **Find the Gen 2 analogues of the six structures in §0** — the block map buffer and its
   border width, the map-attributes block (Gen 2's header), the tileset pointer set, and the
   warp / bg-event / object-event tables — from `pokegold`'s own `ram/wram.asm` and
   `LoadMapAttributes` / `LoadMapDataPointer` call chain. Grade each one, exactly as here.
3. **Establish whether the same "skip the header load" hook exists.** It is the linchpin of
   §3 and it is a *specific* five-instruction shape at the top of one routine; Gen 2 may have
   no equivalent, in which case the recipe is a different one (probably: write the header in
   the window between the warp committing and the attributes load, if such a window exists).
   **This is the question to answer first** — it decides whether the rest is a page or a
   research project.
4. **Re-derive the collision rule.** Gen 2 has a per-tile collision *table* rather than Gen
   1's flat passable-id list, and directional collision. §1.6's bottom-left probe is a Gen 1
   fact and must not be assumed.
5. **Re-measure every ceiling.** Gen 2's block-map buffer, tileset size and object limits are
   all different numbers, and §4's whole value is that its numbers are real.
6. **Build the Gen 2 twin of `warproom_probe`.** The Gold cable-club work already has a save,
   a screen reader and a bot that drives menus, so the harness cost is small; the derivation
   is where the time goes.

**Rough cost, stated as a range because that is honest:** items 1-3 are a day's work if the
skip hook exists and a week if it does not, because "no hook" means finding a different
window and proving it. Items 4-6 are another two to three days on top. The prerequisite —
building `pokegold` at RGBDS 1.0.3 and checking it against `roms.sha1` — is an hour and has
already been done once here.

---

## What this suggests next

* **A room is a destination, so a lobby does not have to be the Cable Club.** The
  [World Trade Centre](../worldlobby.md) is confined to `TRADE_CENTER` because that is where
  the two console objects live — they are hidden events keyed to the map id in ROM
  (`data/events/hidden_events.asm`) and cannot be moved. This page shows the *geometry* half
  is not confined at all: any door in the game can lead to an authored room. What stays
  confined is anything that needs a ROM script — a console to trade at, an NPC to talk to.
  So the natural next shape is a **hub and spokes**: authored rooms anywhere for standing
  about in, and a warp into the real Cable Club when two people want to trade.
* **`wCurMapScriptPtr` and `wCurMapTextPtr` are ours too, and untested.** The probe leaves
  both pointing at the departing room's, which is harmless for a room with no objects. A
  room with a *sign* would need `wCurMapTextPtr` to point at something readable, and that is
  the next thing to try.
* **The same window works for anything `LoadMapData` reads.** `wSpriteSet`, `wNumSprites`
  and the object structs are all written in the skipped section, so a room can be populated
  in exactly the same breath as it is built.
* **The `--no-latch` control is a pattern worth reusing.** Any time a host writes emulated
  memory and hopes it sticks, run it twice and print the diff. It turns "I think this works"
  into ten lines of evidence.

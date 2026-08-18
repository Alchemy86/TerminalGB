"""The published atlas manifest, and where a map lands in it.

Moved here verbatim from swarm_film.py so there is ONE of it.
See chainrender/__init__.py for why this package exists.
"""

import json
import os


class Atlas:
    """PixelGB's Kanto atlas: one image, and where every map sits in it.

    The manifest is the whole of the layout. Nothing here searches, packs or
    guesses — `rect` says where a map is drawn and `tile_px` says how big its
    walk tile is, so `place()` is one multiply and one add.
    """

    def __init__(self, manifest_path, image_path=None,
                 rendering="region-tint", scale=1):
        with open(manifest_path) as f:
            man = json.load(f)
        d = os.path.dirname(os.path.abspath(manifest_path))
        self.cartridge = man.get("cartridge", {})
        self.placement = man.get("placement", {})
        geom_scale = man.get("geometry", {}).get("scale", 1)

        if image_path is None:
            image_path, scale = self._pick_image(man, d, rendering, scale)
        # "the 2x, 4x … images are nearest-neighbour enlargements of this same
        # layout: multiply any rect by the scale" — geometry, atlas.json.
        self.mul = scale / float(geom_scale)
        self.image_path = image_path
        self.scale = scale

        self.origin, self.tile_px, self.kind, self.label = {}, {}, {}, {}
        self.size, self.tiles = {}, {}
        for m in man["maps"]:
            r = m["rect"]
            self.tiles[m["id"]] = (m["tiles"]["width"], m["tiles"]["height"])
            self.origin[m["id"]] = (r["x"] * self.mul, r["y"] * self.mul)
            self.size[m["id"]] = (r["width"] * self.mul, r["height"] * self.mul)
            # Per-entry, as the manifest asks. This atlas happens to be uniform;
            # honouring the field costs a dictionary and means a future atlas
            # that draws interiors smaller cannot silently misplace them.
            self.tile_px[m["id"]] = m["tile_px"] * self.mul
            self.kind[m["id"]] = m["kind"]
            self.label[m["id"]] = m["name"].replace("_", " ").title()

    @staticmethod
    def _pick_image(man, d, rendering, scale):
        want = [im for im in man.get("images", [])
                if im.get("rendering") == rendering and im.get("scale") == scale]
        if not want:
            sys.exit(f"atlas has no {rendering} image at {scale}x; it has "
                     + ", ".join(f"{im['rendering']} {im['scale']}x"
                                 for im in man.get("images", [])))
        p = want[0]["path"]
        # The manifest's paths are the atlas's own publication layout. A local
        # copy may be flattened, so try the stated path, the flattened form and
        # the bare name before giving up — and say which was tried.
        cands = [os.path.join(d, p),
                 os.path.join(d, p.replace("/", "_")),
                 os.path.join(d, os.path.basename(p))]
        cands += [os.path.join(d, f"{os.path.basename(d.rstrip('/'))}_{p.replace('/', '_')}")]
        stem = p.replace("/", "_")
        cands += [os.path.join(d, n) for n in sorted(os.listdir(d))
                  if n.endswith(stem)]
        for c in cands:
            if os.path.exists(c):
                return c, want[0]["scale"]
        sys.exit("atlas image not found; tried:\n  " + "\n  ".join(cands))

    def holds(self, map_id, x, y):
        """Is this tile inside the map it claims to be on?

        **A warp changes `wCurMap` one frame before it changes the
        coordinates**, so on that frame a track says the destination map and
        the *departure's* door tile. Measured on this chain: every one of the
        600 agents carries exactly one row reading `OAKS_LAB (12,11)`, and
        (12,11) is Pallet Town's own `warp_event 12,11, OAKS_LAB, 2` — Oak's
        Lab is ten tiles wide. It is the same trap as the boot-time one
        `chain_tracks.py::settle` handles, arriving mid-track instead of at the
        head, and a hand-placed layout hid it because the stray point landed on
        top of a neighbouring sheet. The atlas has real gaps, so it shows.
        """
        t = self.tiles.get(map_id)
        return t is not None and 0 <= x < t[0] and 0 <= y < t[1]

    def place(self, map_id, x, y):
        """Pixel position of a **fractional** tile coordinate, or `None` when
        the atlas does not place this map.

        **The CENTRE of the tile, not its corner** — `rect` gives a tile's
        top-left, and an agent standing on a tile is in the middle of it.
        Returning the corner and then drawing a sprite centred on it puts every
        agent half a tile up and to the left, which is what "the sprites sit
        left of the path, straddling its edge" was. Measured against the
        cartridge, not eyeballed: see `SPRITE_LIFT`.

        `None` means unplaced and must never be drawn at the origin: the ids a
        track carries include transients the cartridge writes before a map
        header exists, and a battle has no map position at all.
        """
        o = self.origin.get(map_id)
        if o is None:
            return None
        t = self.tile_px[map_id]
        return (o[0] + (x + 0.5) * t, o[1] + (y + 0.5) * t,
                self.kind[map_id] != "outdoor")

    def open_image(self, Image):
        Image.MAX_IMAGE_PIXELS = None      # a region atlas is legitimately huge
        return Image.open(self.image_path).convert("RGB")

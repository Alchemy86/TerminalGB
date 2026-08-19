"""The camera that follows a swarm over a still map.

Lifted out of `swarm_film.py`'s `main()`, where it was two hundred lines of
tuned rules that no other renderer could reach — which is why the swarm view
and AgentGB's compositor have no camera smoothing at all.

Every rule below is unchanged and every constant is the one it was tuned to.
`split_aware=False` reproduces the old behaviour bit for bit, and the harness
asserts that against a recorded trace.

## The six rules (unchanged)

1. THE TARGET IS SMOOTHED, and slowly. `CAM_K` is a ~1.2 s time constant, so
   the camera lags the mass and glides rather than chasing per-frame jitter.
2. A DEADZONE. Below it the camera does not move at all.
3. THE ZOOM IS QUANTISED AND HELD. This was the loudest cause and is easy to
   miss: `scale = VIEW_W / cw` recomputed from a continuously-varying width
   resamples the pixel art at a slightly different ratio *every frame*, so the
   whole picture shimmers whether or not the camera moves. The width snaps to a
   geometric ladder and changes only on hysteresis.
4. THE PAN SPEED IS CAPPED, as a fraction of walking pace so it scales with the
   timelapse factor for free.
5. AXIS BIAS. A vertical corridor wants Y tracked and X nearly fixed.
6. CATCH-UP, KEYED ON THE MASS BEING OUT OF FRAME. Rules 1-5 are calibrated for
   *jitter*; when the swarm genuinely walks somewhere else they are calibrated
   for the wrong thing. `outside` is exactly zero whenever the swarm is already
   on screen, so this is provably a no-op on every frame that was already right.

## Rule 7, and what it fixes

**A percentile is discontinuous when the swarm is bimodal, and the training
legs are bimodal by construction** — agents split between the Pokemon Center
and the grass, and the healed ones walk back. Two things then go wrong at once,
and both were reported as "it juggles back and forth":

* `spread()` returned the **midpoint of the 5th and 95th percentiles** as the
  centre. With two clusters that midpoint sits in the *empty gap between them*,
  and it lurches every time the population balance crosses the percentile
  boundary — a camera pointed at nobody, moving because of agents it is not
  showing.
* the same span drives the zoom, so the width is pinned to "both clusters" and
  can never tighten when they regroup. Measured, this is the loud one: over the
  training stretch of `chain-film` the crop width was a chattering square wave.

The fix is to smooth the **box** and not only the camera: an EMA on each edge
of the percentile box before any of rules 1-6 sees it. That removes the
discontinuity at its source rather than downstream of it, which is why raising
`CAM_K` or the deadzone never worked — they were being fed a step function.

Measured on `chain-film` (24 agents, 15,632 frames at --speed 12 --fps 60),
`--no-split-aware` against the default:

    direction reversals   1.39/s x, 0.93/s y  ->  0.17/s x, 0.43/s y
    frames camera still   78.2% x, 71.1% y    ->  85.2% x, 82.1% y
    largest one-frame pan 48 px x, 48 px y    ->  11 px x,  4 px y
    resample ratio moved  140 frames (0.9%)   ->  19 frames (0.1%)
    frames below 90% cov  5006 (32.0%)        ->  4744 (30.3%)

Two figures move the other way and both are stated rather than buried. The
**distinct crop width count** goes 18 -> 20 because the smoothed box passes
continuously through more rungs of the zoom ladder instead of jumping between
two — the metric that matters for shimmer is how often the resample ratio
actually moved, and that is six times better. And **mean coverage** goes
93.0% -> 92.3% while the *below-90%* count improves: the distribution tightened
rather than shifted.

**34.2% of these frames are geometrically impossible** — the camera may not
widen past the world it crops from, and on those the swarm genuinely does not
fit in the widest view there is. `report_impossible()` prints that figure; no
camera rule can beat it, so nothing here promises past it.

"""

import math


class Camera:
    """Follows a swarm. One `step()` per output frame, in order."""

    # --- rules 1-5, unchanged constants ---
    CAM_K = 0.028          # EMA per frame: tau ~= 36 frames ~= 1.2 s at 30 fps
    CAM_DEAD = 0.16        # of the viewport, per axis, before anything moves
    CAM_X_TRACK = 0.22     # how much of the X drift to follow at all
    CAM_STEP = 1.10        # zoom ladder ratio
    CAM_GROW = 1.06        # widen only when the mass needs this much more
    CAM_SHRINK = 0.74      # tighten only when it needs this much less
    CAM_CATCHUP = 14.0     # how much faster it may glide when the mass is out
    CAM_UNBIAS = 4.0       # how fast the X bias is released when it is out
    CAM_INSET = 0.06       # keep the mass comfortably inside, not on the edge

    # --- rule 7 ---
    # The band the box is taken over. **Measured on `chain-film` this is not
    # the lever** — 5-95, 2-98 and 0-100 give the same figures to the digit at
    # 24 agents, because the box's own EMA below washes out the one or two
    # agents the clip discards. It is widened anyway because at 600 agents 5-95
    # discards thirty per edge rather than one, and because "keep everyone on
    # screen" was half the ask.
    BAND = (0.02, 0.98)
    OLD_BAND = (0.05, 0.95)
    # The box edges' own EMA. Slower than the camera: the box is what the
    # camera is *aiming at*, so it has to be the steadier of the two, or the
    # smoothing simply moves the jitter one level up.
    #
    # **These two are re-measured, not inherited.** The pair that was in flight
    # (0.012 / 0.10) is smoother than no rule 7 at all but grows the box fast
    # enough to sweep the zoom ladder, which on a single-leg corpus costs more
    # zoom changes than it saves. Swept on `chain-film` (24 agents, the whole
    # chain, bimodal) and on 40 tracks of link 11 (one leg, unimodal), against
    # the pre-fix camera:
    #
    #     BOX_K  GROW | chain-film reversals x/y   still x/y   resample changes
    #     pre-fix     |        1.39 / 0.93        78.2 / 71.1        140
    #     0.012  0.10 |        0.35 / 0.52        84.1 / 75.5         27
    #     0.006  0.10 |        0.25 / 0.49        85.7 / 78.9         23
    #     0.006  0.03 |        0.17 / 0.43        85.2 / 82.1         19
    #     0.006  0.006|        0.13 / 0.22        85.0 / 86.8         19   <- but
    #                                             coverage below 90% 32.6 -> 38.0%
    #
    # The last row is the trade this pair is chosen against: a box that never
    # grows quickly is beautifully steady and **drops agents**, because rule 6
    # keys its catch-up on how far the box pokes past the frame and a slow box
    # is slow to notice. 0.006 / 0.03 is better than the pre-fix camera on
    # every column on both corpora except one — one leg's zoom changes go
    # 18 -> 23 of 50,000 frames, i.e. 0.04% -> 0.05%.
    BOX_K = 0.006
    # A cluster that genuinely leaves must not be waited for indefinitely, so
    # the box may always widen faster than it narrows -- the same asymmetry
    # rule 3 uses, for the same reason.
    BOX_K_GROW = 0.03

    # **Rule 8: every rung of the ladder is an INTEGER pixel scale, and that is
    # a compression finding as much as a sharpness one.**
    #
    # The geometric ladder above snaps the width so the resample ratio holds
    # still — but it never asked what that ratio IS, and measured over the Mt
    # Moon cut **not one frame of 7,100 sat at an integer scale**: the two
    # widths the film spends most of its time at are 0.4025x and 2.4615x.
    #
    # The source is pixel art. Gen 1 draws grass, cave floor and water as a
    # ONE-PIXEL dither checkerboard, so a fractional nearest resample turns
    # that checkerboard into moiré: an irregular, frame-to-frame-unstable
    # high-frequency pattern over the whole picture. It looks soft and
    # shimmery — which is what "often a tab blurry" is — and it is also the
    # single most expensive thing anybody can hand a video codec, because
    # every transform block is full of noise that never repeats. Both halves
    # of "the files are huge and it looks blurry" are this.
    #
    # At an integer scale nearest is exact: an upscale by `s` makes every
    # source pixel an aligned `s x s` block, a downscale by `k` samples one
    # pixel in `k` on a regular grid. The dither stays a dither, the codec's
    # blocks line up with the content, and nothing shimmers.
    #
    # The cost is a coarser ladder — the rungs are 1/4, 1/3, 1/2, 1, 2, 3, 4
    # rather than 10% apart — so it is rounded UP (a wider view, a smaller
    # scale) and never down: showing more world costs nothing, and losing
    # agents off the edge is the one thing this camera must not do.
    INT_UP = (1, 2, 3, 4, 6, 8)      # upscale by s: crop is view_w / s
    INT_DOWN = (2, 3, 4, 6, 8, 12)   # downscale by k: crop is view_w * k

    # **Rule 9: the zoom TRAVELS. It never cuts.**
    #
    # Reported in the captain's own words: *"the videos are supposed to follow
    # the majority and be smooth but it suddenly jerks OUT when more
    # entertaining say mt moon, instead of either panning out and across
    # nicely"*. Rule 7 fixed the shape rule 3 could not handle — a swarm split
    # between two clusters on ONE map — and a MAP TRANSITION is a third shape
    # that neither covers.
    #
    # What happens at one: the atlas places Mt Moon 1F 1,400 px from Route 4,
    # so the instant the first agents warp in, the box spans both and `want`
    # multiplies. Rule 3 then assigns `cw = quantise(want)` in a single frame,
    # which on an integer ladder can be **three rungs at once** — an 8x jump
    # between one output frame and the next. That is a cut, and a cut is what
    # a viewer calls a jerk.
    #
    # So the zoom moves at most ONE rung per change and then has to hold. The
    # centre was already rate-limited (rule 4's pan cap, opened up by rule 6);
    # this is the same treatment for the other axis of the shot, and the two
    # together are what "pans out and across" means. Widening is allowed to
    # step faster than narrowing, the same asymmetry rules 3 and 7 already
    # use and for the same reason: rule 6 keys its catch-up on how far the
    # mass pokes outside, so a camera slow to widen is slow to notice it has
    # lost people.
    ZOOM_DWELL_OUT = 6     # output frames a widening rung must be held
    ZOOM_DWELL_IN = 24     # and a narrowing one, which is never urgent

    # **And it GLIDES between rungs rather than stepping onto them**, because
    # one rung of an integer ladder is still a 1.33x-2x jump and a 2x jump in
    # one frame is a cut. So the width travels geometrically towards the rung
    # it is heading for and settles exactly on it:
    #
    #   **the picture is pixel-exact whenever the camera is at rest, and only
    #   the handful of frames of an actual zoom are fractionally scaled.**
    #
    # That is the whole trade, stated: rule 8 wants integer scale for
    # crispness and compression, a viewer wants a continuous zoom, and a
    # moving picture is where the moire is least visible and a still one is
    # where it is most. Measured on the Mt Moon cut, five zoom changes in
    # 7,100 frames, so under 2% of frames are ever off a rung.
    ZOOM_RATE = 1.035      # per output frame; 2x takes ~20 frames

    def __init__(self, view_w, out_h, min_w, world_w, world_h, walk_px,
                 split_aware=True, integer_zoom=True):
        self.view_w, self.out_h = view_w, out_h
        self.min_w = min_w
        self.world_w, self.world_h = world_w, world_h
        self.split_aware = split_aware
        self.integer_zoom = integer_zoom
        # The rungs, as crop widths, smallest first. Built once so `quantise`
        # is a lookup and the set is inspectable.
        self.rungs = sorted({float(view_w) / s for s in self.INT_UP}
                            | {float(view_w) * k for k in self.INT_DOWN})
        # **The pan cap is a fraction of WALKING PACE, not a pixel count.** At
        # 9x each output frame covers nine times the ground, so a cap tuned at
        # 1x cannot keep up with its own subject.
        self.cam_max_pan = 0.85 * walk_px
        self.prev = None      # (x, y, width) floats
        self.box = None       # (lo_x, hi_x, lo_y, hi_y), smoothed
        self.held = 0         # output frames since the zoom last moved a rung
        self.zoom_to = None   # the rung the width is travelling towards
        self.band = self.BAND if split_aware else self.OLD_BAND

    # --- the box -------------------------------------------------------------
    def _percentiles(self, vals):
        vals = sorted(vals)
        n = len(vals)
        lo = vals[max(0, int(self.band[0] * n))]
        hi = vals[min(n - 1, int(self.band[1] * n))]
        return lo, hi

    def _smooth_box(self, raw):
        """Rule 7. An EMA per edge, widening faster than it narrows."""
        if self.box is None or not self.split_aware:
            self.box = raw
            return raw
        out = []
        for i, (was, now) in enumerate(zip(self.box, raw)):
            # edges 0 and 2 are the low ends: growing means going DOWN
            growing = (now < was) if i in (0, 2) else (now > was)
            k = self.BOX_K_GROW if growing else self.BOX_K
            out.append(was + (now - was) * k)
        self.box = tuple(out)
        return self.box

    def quantise(self, w):
        """Snap a wanted width onto a fixed geometric ladder.

        Anchored on `min_w` rather than on the frame, so the same wanted width
        always yields the same exact scale -- the property that stops the
        resample ratio moving under the picture.
        """
        if self.integer_zoom:
            # Round UP to the next integer-scale rung, so the view only ever
            # gets wider than asked for — see rule 8 above.
            for cw in self.rungs:
                if cw >= w - 1e-6:
                    return cw
            return self.rungs[-1]
        k = max(0, round(math.log(max(w, self.min_w) / self.min_w)
                         / math.log(self.CAM_STEP)))
        return self.min_w * (self.CAM_STEP ** k)

    def next_rung(self, now, target):
        """One step along the ladder from `now` towards `target`.

        With no ladder (integer zoom off) the rungs are 10% apart and a single
        step is imperceptible, so the geometric case steps by one `CAM_STEP`
        rather than jumping — the rule is about how FAR the zoom may move in
        one frame, not about which ladder it is on.
        """
        if target == now:
            return now
        if self.integer_zoom:
            rungs = self.rungs
            if target > now:
                nxt = [r for r in rungs if r > now * 1.0001]
                return min(nxt) if nxt else now
            nxt = [r for r in rungs if r < now * 0.9999]
            return max(nxt) if nxt else now
        step = self.CAM_STEP if target > now else 1.0 / self.CAM_STEP
        moved = now * step
        return min(moved, target) if target > now else max(moved, target)

    # --- one frame -----------------------------------------------------------
    def step(self, xs, ys, live=True):
        """Advance one output frame. Returns (cx, cy, cw), cx/cy rounded.

        `live` is whether any agent is on screen this frame, and it is separate
        from `xs` being non-empty because an arrival marker contributes to the
        box without being a live agent — the original loop tested the live
        population, so a frame with only markers holds its target rather than
        re-aiming at them. **`self.box` persists across such a frame**, so the
        catch-up rule keeps measuring against the last real box rather than
        going blind.
        """
        if live and xs:
            lx, hx = self._percentiles(xs)
            ly, hy = self._percentiles(ys)
            lx, hx, ly, hy = self._smooth_box((lx, hx, ly, hy))
            tx, ty = (lx + hx) / 2, (ly + hy) / 2
            want = max(self.min_w, (hx - lx) * 1.25,
                       (hy - ly) * 1.25 * self.view_w / self.out_h)
        elif self.prev:
            tx, ty, want = self.prev
        else:
            tx, ty, want = self.world_w / 2, self.world_h / 2, self.min_w
        want = min(want, self.world_w * 1.0)
        box = self.box

        if self.prev is None:
            self.prev = (float(tx), float(ty), self.quantise(want))
            return (round(self.prev[0]), round(self.prev[1]), self.prev[2])

        px, py, pw = self.prev
        # (3) hold the zoom unless the mass genuinely outgrew or fell well
        # inside it, then snap to the ladder -- (9) ONE RUNG AT A TIME, and
        # only after the last one has been held.
        # **Rules 8 and 9 are one switch**, so `--no-integer-zoom` reproduces
        # the pre-rule-8 camera exactly and the equality gate keeps its
        # meaning. Nothing is lost by pairing them: without the integer ladder
        # the rungs are 10% apart, and rate-limiting a 10% step is
        # rate-limiting something already imperceptible.
        if not self.integer_zoom:
            cw = pw
            if want > pw * self.CAM_GROW or want < pw * self.CAM_SHRINK:
                cw = self.quantise(want)
            return self._pan(px, py, tx, ty, box,
                             min(cw, self.world_w * 1.0))
        self.held += 1
        if self.zoom_to is None and (want > pw * self.CAM_GROW
                                     or want < pw * self.CAM_SHRINK):
            target = self.quantise(want)
            out = target > pw
            if self.held >= (self.ZOOM_DWELL_OUT if out else self.ZOOM_DWELL_IN):
                nxt = self.next_rung(self.quantise(pw), target)
                if abs(nxt - pw) > 1e-6:
                    self.zoom_to = nxt
        cw = pw
        if self.zoom_to is not None:
            if self.zoom_to > pw:
                cw = min(self.zoom_to, pw * self.ZOOM_RATE)
            else:
                cw = max(self.zoom_to, pw / self.ZOOM_RATE)
            if abs(cw - self.zoom_to) < 1e-6:
                cw, self.zoom_to, self.held = self.zoom_to, None, 0
        cw = min(cw, self.world_w * 1.0)
        return self._pan(px, py, tx, ty, box, cw)

    def _pan(self, px, py, tx, ty, box, cw):
        """Rules 1, 2, 4, 5 and 6 — where the camera moves, given a width."""
        chh = cw * self.out_h / self.view_w

        # (6) how far the mass pokes past the frame it is being shown in,
        # measured against the view the camera has *now*, before it moves.
        outside = 0.0
        if box:
            ix, iy = cw * (0.5 - self.CAM_INSET), chh * (0.5 - self.CAM_INSET)
            outside = max(
                0.0,
                (px - ix - box[0]) / cw, (box[1] - px - ix) / cw,
                (py - iy - box[2]) / chh, (box[3] - py - iy) / chh,
            )

        # (2) deadzone, per axis -- but only while the mass is inside the
        # frame. A deadzone that holds a camera still while the swarm is off
        # screen is not suppressing jitter, it is refusing to look.
        dx, dy = tx - px, ty - py
        if outside <= 0.0:
            if abs(dx) < cw * self.CAM_DEAD:
                dx = 0.0
            if abs(dy) < chh * self.CAM_DEAD:
                dy = 0.0
        # (5) axis bias, released by (6) when the mass has left sideways.
        dx *= min(1.0, self.CAM_X_TRACK + self.CAM_UNBIAS * outside)
        # (1) smooth, then (4) cap -- both opened up by (6). **The gain has to
        # rise, not only the cap**: `CAM_K` is 2.8% of the error per frame, so
        # a bigger cap alone changes nothing while the EMA is what binds.
        gain = 1.0 + self.CAM_CATCHUP * outside
        cap = self.cam_max_pan * gain
        vx = max(-cap, min(cap, dx * self.CAM_K * gain))
        vy = max(-cap, min(cap, dy * self.CAM_K * gain))
        # **The accumulator stays a float and only the OUTPUT is rounded.**
        # Rounding the accumulator looks like the same thing and is a deadlock:
        # a drift just outside the deadzone yields a step well under half a
        # pixel, `round` throws it away, and the camera is stuck for ever.
        self.prev = (px + vx, py + vy, cw)
        # (3, again) whole world pixels, so the crop lands on the same lattice
        # every frame and a held camera is bit-identical frame to frame.
        return (round(self.prev[0]), round(self.prev[1]), cw)

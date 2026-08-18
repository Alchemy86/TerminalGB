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
  can never tighten when they regroup.

The fix is to smooth the **box** and not only the camera: an EMA on each edge
of the percentile box before any of rules 1-6 sees it. That removes the
discontinuity at its source rather than downstream of it, which is why raising
`CAM_K` or the deadzone never worked — they were being fed a step function.
The band widens too (`BAND`), because the ask was to keep everyone on screen
and the 5-95 clip is what was throwing agents out of frame.
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
    # The band the box is taken over. 5-95 discards a tenth of the swarm by
    # construction, which is most of what "agents keep leaving the frame" was.
    BAND = (0.02, 0.98)
    OLD_BAND = (0.05, 0.95)
    # The box edges' own EMA. Slower than the camera: the box is what the
    # camera is *aiming at*, so it has to be the steadier of the two or the
    # smoothing simply moves the jitter one level up.
    BOX_K = 0.012
    # A cluster that genuinely leaves must not be waited for indefinitely, so
    # the box may always widen faster than it narrows -- the same asymmetry
    # rule 3 uses, for the same reason.
    BOX_K_GROW = 0.10

    def __init__(self, view_w, out_h, min_w, world_w, world_h, walk_px,
                 split_aware=True):
        self.view_w, self.out_h = view_w, out_h
        self.min_w = min_w
        self.world_w, self.world_h = world_w, world_h
        self.split_aware = split_aware
        # **The pan cap is a fraction of WALKING PACE, not a pixel count.** At
        # 9x each output frame covers nine times the ground, so a cap tuned at
        # 1x cannot keep up with its own subject.
        self.cam_max_pan = 0.85 * walk_px
        self.prev = None      # (x, y, width) floats
        self.box = None       # (lo_x, hi_x, lo_y, hi_y), smoothed
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
        k = max(0, round(math.log(max(w, self.min_w) / self.min_w)
                         / math.log(self.CAM_STEP)))
        return self.min_w * (self.CAM_STEP ** k)

    # --- one frame -----------------------------------------------------------
    def step(self, xs, ys):
        """Advance one output frame. Returns (cx, cy, cw), cx/cy rounded."""
        box = None
        if xs:
            lx, hx = self._percentiles(xs)
            ly, hy = self._percentiles(ys)
            box = self._smooth_box((lx, hx, ly, hy))
            lx, hx, ly, hy = box
            tx, ty = (lx + hx) / 2, (ly + hy) / 2
            want = max(self.min_w, (hx - lx) * 1.25,
                       (hy - ly) * 1.25 * self.view_w / self.out_h)
        elif self.prev:
            tx, ty, want = self.prev
        else:
            tx, ty, want = self.world_w / 2, self.world_h / 2, self.min_w
        want = min(want, self.world_w * 1.0)

        if self.prev is None:
            self.prev = (float(tx), float(ty), self.quantise(want))
            return (round(self.prev[0]), round(self.prev[1]), self.prev[2])

        px, py, pw = self.prev
        # (3) hold the zoom unless the mass genuinely outgrew or fell well
        # inside it, then snap to the ladder.
        cw = pw
        if want > pw * self.CAM_GROW or want < pw * self.CAM_SHRINK:
            cw = self.quantise(want)
        cw = min(cw, self.world_w * 1.0)
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

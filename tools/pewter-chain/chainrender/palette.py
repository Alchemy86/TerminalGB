"""Every colour any renderer uses, in one place.

There were four renderers and three of them defined their own swatches, so a
palette fix landed on one video and not the others. That is the whole reason
this package exists.

## Two arrow encodings, and why both are kept

`traffic` is the original: **hue and size both carry how many DISTINCT AGENTS
stepped off the tile**, argued at length in `docs/agent/path-arrows.md`. Step
count is measurably the wrong unit here (one grinding band scores 20,529 steps
against a corridor the whole swarm walked once at 24), and distinct agents is
bounded by the swarm size so the ramp spends its range on the question asked.

`direction` is the encoding Peter Whidden's video uses (`DcYLT37ImBY`), which
is what a reader means by "the arrows glow rainbow like": **hue is the
direction of travel** — a north-south corridor comes out one solid colour and
reads as a route at a glance, where a sequential ramp over the same tiles is a
wash of near-neighbours.

**Neither encoding loses the other quantity**, which is this repository's own
rule that every encoding is readable a second way. Under `direction`, traffic
moves to **brightness and size**: a tile one agent crossed is dim and small, a
tile the whole swarm crossed is vivid and large. Under `traffic`, direction is
still carried by the arrow's own shape. So a picture drawn either way survives
being printed small, and in grey.
"""

# --- the world layer ---------------------------------------------------------
BG = (16, 18, 22)
PANEL = (24, 27, 33)
INK = (226, 230, 238)
DIM = (140, 148, 162)

# **The world stays grey and the agents carry the colour.** That is what makes
# a crowd readable without labels: the eye separates agent from world instantly
# and the colour carries the meaning. A Game Boy frame is four shades already,
# so nothing has to be desaturated to get there.
#
# **The hue is the heat map, read at the agent's own feet** — how many
# agent-frames the swarm has already spent on the tile it is standing on. Not
# progress: novelty. Standing where thousands have already been is one end of
# the scale, standing on rarely-trodden ground is the other, so **the frontier
# lights up on its own** and an agent doing something genuinely new stands
# apart from the crowd milling around Pallet Town.
#
# It is the same visitation count `corpus_report.py` draws its heat maps from —
# one computation, one source of truth — and it is count-based exploration
# novelty made visible: the colour *is* the signal an RL agent would be
# rewarded on.
#
# **This is a declared exception to the categorical colour guard**, not a
# silent one. That guard says no colour may appear that was not in the source
# framebuffer, and it still applies **to the map layer**: the map art is
# reproduced exactly. The agent overlay is deliberately outside it.
HEAT_HUE = [
    (70, 110, 210), (70, 130, 215), (70, 150, 220), (70, 175, 215),
    (70, 195, 195), (80, 205, 160), (110, 210, 120), (150, 215, 90),
    (195, 210, 70), (225, 195, 65), (240, 165, 60), (245, 130, 60),
    (250, 100, 70), (255, 80, 110),
]

# **`wIsInBattle` is not a boolean and a catch-all arm over it is a defect.**
# The track carries `$D057` verbatim: 1 is a wild encounter, 2 is a trainer,
# and **`$FF` means the player blacked out** — which is the end of the run, not
# a fight (`plugins/agent`'s own reader had exactly this bug, and displayed a
# whole stale battle panel in the overworld because of it). Anything that is
# not 1 or 2 is therefore drawn as *not fighting* rather than swept into the
# truthy branch. Measured over the corpus, only 0, 1 and 2 occur today; the
# point is that the fourth value cannot quietly become a battle marker.
BATTLE_KIND = {1: "wild", 2: "trainer"}
# Two colours because the two are different events, and on this chain one of
# them is a story beat: every trainer battle in links 01-11 is the rival in
# Oak's lab, and the swarm turning red together in one room is the picture.
BATTLE_HUE = {"wild": (255, 90, 90), "trainer": (255, 170, 60)}

# --- distinct agents, sequential, cool to warm -------------------------------
# **Sequential, cool to warm, and it is DISTINCT AGENTS.** Deliberately its own
# ramp rather than `HEAT_HUE` above: that one is agent-*frames* and is read
# inverted (blue = heavily trodden, warm = novel, so an agent doing something
# new lights up). Reusing the same swatches for the opposite polarity and a
# different unit would be the worst of both. Ten bands, because a swarm of 24
# has 24 possible values and ten is as many as an eye separates in a legend.
TRAFFIC_RAMP = [
    (64, 96, 176), (62, 124, 188), (60, 152, 190), (64, 178, 176),
    (86, 196, 140), (132, 204, 100), (186, 204, 76), (226, 186, 68),
    (243, 142, 62), (248, 88, 84),
]

# --- direction ---------------------------------------------------------------
# Indexed by `chainrender.arrows.DIRS` — down, up, left, right.
#
# Four hues a quarter turn apart on the colour wheel, so no two are confusable
# at arrow size, and every one is light enough to carry over the atlas's dark
# grey. Read off Whidden's own frame, where the long north-south corridor is
# solid cyan, the eastward bands are green, the westward ones pink and the
# southward ones orange.
#
# **They are also separated in LIGHTNESS, not only in hue** (relative luminance
# 0.55 / 0.75 / 0.50 / 0.72), so the picture still distinguishes them printed in
# grey and for a red-green colour-blind reader — which the plain hue wheel that
# was tried first did not.
DIRECTION_HUE = {
    0: (255, 150, 86),    # down  — orange
    1: (95, 224, 240),    # up    — cyan
    2: (255, 111, 200),   # left  — pink
    3: (123, 232, 107),   # right — green
}

# A paced tile — one the swarm walked both ways, drawn double-headed — is a
# lilac that belongs to no single direction, because by construction it has no
# one direction. Whidden's frame carries the same fifth colour.
PACED_HUE = (186, 156, 255)

# How far a single-agent tile is dimmed towards the background before the
# traffic term lifts it back. Deliberately not near zero: a tile one agent
# walked is still evidence, and 232 of `chain-film`'s 978 tiles carry exactly
# one agent — a floor low enough to hide them hides a quarter of the picture.
DIM_FLOOR = 0.58

# The smallest an arrow is drawn, as a fraction of the whole-swarm arrow.
#
# Under `traffic` this is the original 0.46, and it has to be: hue is the only
# other channel there, so size must span the whole range for the redundancy
# rule to hold. Under `direction` the hue is spent on the direction and
# **brightness** carries the traffic instead, so the floor can rise — and it
# has to. Measured on `chain-film`, 78% of tiles sit in the bottom four of the
# ten traffic bands, so at 0.46 most of the picture is specks, which is half of
# what "grey and near impossible to see" was. The other half was hue.
SIZE_FLOOR = {"traffic": 0.46, "direction": 0.64}


def band_of(k, n_agents):
    """Which ramp band a distinct-agent count falls in.

    Linear, because the quantity is bounded by the swarm size and is not
    heavy-tailed — unlike dwell, which is, and which is why the heat maps
    log-scale.
    """
    if n_agents <= 1:
        return len(TRAFFIC_RAMP) - 1
    t = (k - 1) / (n_agents - 1)
    return max(0, min(len(TRAFFIC_RAMP) - 1,
                      int(round(t * (len(TRAFFIC_RAMP) - 1)))))


def traffic_fraction(k, n_agents):
    """0.0 for one agent, 1.0 for the whole swarm. The size and brightness
    term, kept separate from `band_of` so it is smooth rather than banded."""
    if n_agents <= 1:
        return 1.0
    return max(0.0, min(1.0, (k - 1) / (n_agents - 1)))


def arrow_rgb(encoding, d_ix, k, n_agents, paced=False):
    """The colour of one arrow, under whichever encoding is in force.

    `traffic` reproduces the original exactly, so a picture drawn with it is
    bit-identical to every one published before this package existed.
    """
    if encoding == "traffic":
        return TRAFFIC_RAMP[band_of(k, n_agents)]
    base = PACED_HUE if paced else DIRECTION_HUE[d_ix]
    # Traffic as brightness: blend from a dimmed version of the hue up to the
    # hue at full strength. The dim end keeps the hue (so direction is still
    # readable on a tile one agent crossed) and only loses intensity.
    t = DIM_FLOOR + (1.0 - DIM_FLOOR) * traffic_fraction(k, n_agents)
    return tuple(int(round(BG[i] + (base[i] - BG[i]) * t)) for i in range(3))


def legend_swatches(encoding):
    """(label, rgb) pairs for the panel, so the legend cannot disagree with
    what was drawn."""
    if encoding == "traffic":
        return [("1", TRAFFIC_RAMP[0]), ("", TRAFFIC_RAMP[len(TRAFFIC_RAMP) // 2]),
                ("all", TRAFFIC_RAMP[-1])]
    return [("up", DIRECTION_HUE[1]), ("down", DIRECTION_HUE[0]),
            ("left", DIRECTION_HUE[2]), ("right", DIRECTION_HUE[3]),
            ("both ways", PACED_HUE)]


def arrow_scale(encoding, k, n_agents):
    """How big one arrow is drawn, 0..1 of the whole-swarm arrow."""
    lo = SIZE_FLOOR[encoding]
    return lo + (1.0 - lo) * traffic_fraction(k, n_agents)


ENCODINGS = ("direction", "traffic")

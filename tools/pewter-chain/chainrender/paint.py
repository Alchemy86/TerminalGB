"""Drawing primitives shared by every renderer.

Moved here verbatim from swarm_film.py + path_arrows.py so there is ONE of it.
See chainrender/__init__.py for why this package exists.
"""

import os


def tint(sprite, rgb, Image):
    """Recolour a four-shade sprite onto one hue, keeping its own shading and
    its alpha. Nothing is invented: the luminance ramp is the sprite's."""
    px = sprite.load()
    out = Image.new("RGBA", sprite.size)
    op = out.load()
    for y in range(sprite.height):
        for x in range(sprite.width):
            r, g, b, al = px[x, y]
            if al == 0:
                op[x, y] = (0, 0, 0, 0)
                continue
            # 0 = the sprite's darkest ink, 1 = its lightest.
            t = r / 255.0
            op[x, y] = (
                int(rgb[0] * (0.35 + 0.65 * t)),
                int(rgb[1] * (0.35 + 0.65 * t)),
                int(rgb[2] * (0.35 + 0.65 * t)),
                al,
            )
    return out

def wrap_hud(text, font, width):
    """Break `text` onto as many lines as the HUD column can hold."""
    out, line = [], ""
    for word in text.split():
        trial = f"{line} {word}".strip()
        if line and font.getbbox(trial)[2] > width:
            out.append(line)
            line = word
        else:
            line = trial
    if line:
        out.append(line)
    return out

def on_screen(px, py, x0, y0, cw, ch):
    """Is any of this agent's sprite in frame?

    **Judged by the sprite, not by its centre.** A film's off-frame warning is
    a claim about what a viewer can see, and an agent one tenth of a pixel past
    the boundary is drawn, plainly, half in frame. Counting it as lost made a
    knife-edge camera position report 362 of 592 agents missing from a picture
    that showed nearly all of them — the number was wrong, not the shot.
    """
    h = TILE / 2.0
    return (x0 - h <= px <= x0 + cw + h) and (y0 - h <= py <= y0 + ch + h)

def load_font(px):
    from PIL import ImageFont
    for f in ("/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Bold.ttf",
              "/usr/share/fonts/google-noto-vf/NotoSansMono[wght].ttf"):
        if os.path.exists(f):
            try:
                return ImageFont.truetype(f, px)
            except Exception:
                pass
    return ImageFont.load_default()

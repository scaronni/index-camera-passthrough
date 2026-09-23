#!/usr/bin/env python3
"""Draw the splash image shown while the cameras start, in the colors of the SteamVR
settings.

Usage: python3 docs/splash.py splash.png

Needs Pillow, and the variable Noto Sans font, found with fc-match.
"""

import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

# Colors of the SteamVR settings: background, buttons, text, secondary text, and the
# "Show" button as an accent.
BACKGROUND, CARD, TEXT, SECONDARY, ACCENT = "#0a0f14", "#23262e", "#dee2e5", "#c0c0c0", "#7b6980"
TITLE, STATUS = "Index Camera Passthrough", "Starting the cameras…"
# The image is the left and right eye side by side, each is this size.
EYE = 960
# Drawn at this scale, then scaled down for smooth edges.
SCALE = 2


def font(path, size, weight):
    f = ImageFont.truetype(path, size * SCALE)
    f.set_variation_by_axes([weight])
    return f


def main(output):
    path = subprocess.run(
        ["fc-match", "-f", "%{file}", "Noto Sans"], check=True, capture_output=True, text=True
    ).stdout
    title_font, status_font = font(path, 30, 600), font(path, 24, 400)

    size = EYE * SCALE
    eye = Image.new("RGB", (size, size), BACKGROUND)
    draw = ImageDraw.Draw(eye)
    center = size // 2
    card_width = int(draw.textlength(TITLE, font=title_font)) + 2 * 40 * SCALE
    card_height = 160 * SCALE
    draw.rounded_rectangle(
        (
            center - card_width // 2,
            center - card_height // 2,
            center + card_width // 2,
            center + card_height // 2,
        ),
        radius=12 * SCALE,
        fill=CARD,
    )
    draw.text((center, center - 27 * SCALE), TITLE, font=title_font, fill=TEXT, anchor="mm")
    draw.rounded_rectangle(
        (center - 36 * SCALE, center + 1 * SCALE, center + 36 * SCALE, center + 5 * SCALE),
        radius=2 * SCALE,
        fill=ACCENT,
    )
    draw.text((center, center + 32 * SCALE), STATUS, font=status_font, fill=SECONDARY, anchor="mm")
    eye = eye.resize((EYE, EYE), Image.LANCZOS)

    splash = Image.new("RGBA", (EYE * 2, EYE))
    splash.paste(eye, (0, 0))
    splash.paste(eye, (EYE, 0))
    splash.save(output, optimize=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])

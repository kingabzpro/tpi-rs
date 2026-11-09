"""Render a captured ANSI terminal session to a PNG screenshot.

Usage:
    tpi tui ... > reports/tui_raw.txt
    python scripts/ansi2png.py reports/tui_raw.txt docs/screenshots/tui.png --cols 120 --rows 30

The last screen state in the capture is drawn, so a run killed by timeout still
produces the frame it was showing. Needs pyte and Pillow.
"""

import argparse
import sys

import pyte
from PIL import Image, ImageDraw, ImageFont

# Windows Terminal "Campbell" palette, which is what the docs screenshots use.
PALETTE = {
    "black": "#0C0C0C",
    "red": "#C50F1F",
    "green": "#13A10E",
    "yellow": "#C19C00",
    "blue": "#0037DA",
    "magenta": "#881798",
    "cyan": "#3A96DD",
    "white": "#CCCCCC",
    "brightblack": "#767676",
    "brightred": "#E74856",
    "brightgreen": "#16C60C",
    "brightyellow": "#F9F1A5",
    "brightblue": "#3B78FF",
    "brightmagenta": "#B4009E",
    "brightcyan": "#61D6D6",
    "brightwhite": "#F2F2F2",
}
DEFAULT_FG = PALETTE["white"]
DEFAULT_BG = PALETTE["black"]


ORDER = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "brightblack", "brightred", "brightgreen", "brightyellow",
    "brightblue", "brightmagenta", "brightcyan", "brightwhite",
]
# pyte resolves 256-colour SGR to hex, so map the first 16 back to Campbell.
HEX_INDEX = {value: index for index, value in enumerate(pyte.graphics.FG_BG_256)}


def color(value, fallback):
    if isinstance(value, str):
        name = value.replace(" ", "").lower()
        if name in PALETTE:
            return PALETTE[name]
        if name == "default":
            return fallback
        index = HEX_INDEX.get(name)
        if index is not None:
            return PALETTE[ORDER[index]] if index < 16 else "#" + name
    if isinstance(value, int):
        if 0 <= value < 16:
            return PALETTE[ORDER[value]]
        if 16 <= value < 256:
            return "#" + pyte.graphics.FG_BG_256[value]
    return fallback


def render(
    raw: str,
    cols: int,
    rows: int,
    font_path: str,
    size: int,
    pad: int,
    screen_rows: int = 0,
    screen_cols: int = 0,
):
    # A screen wider than the drawn area stops the capture from wrapping at --cols.
    screen = pyte.Screen(screen_cols or cols, screen_rows or rows)
    stream = pyte.Stream(screen)
    stream.feed(raw)

    font = ImageFont.truetype(font_path, size)
    cell_w = int(round(max(font.getlength(c) for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789."))) + 1
    ascent, descent = font.getmetrics()
    cell_h = ascent + descent

    image = Image.new("RGB", (cols * cell_w + pad * 2, rows * cell_h + pad * 2), DEFAULT_BG)
    draw = ImageDraw.Draw(image)

    for y in range(rows):
        row = screen.buffer[y]
        for x in range(cols):
            char = row[x]
            box = (
                pad + x * cell_w,
                pad + y * cell_h,
                pad + (x + 1) * cell_w - 1,
                pad + (y + 1) * cell_h - 1,
            )
            background = color(char.bg, DEFAULT_BG)
            if background != DEFAULT_BG:
                draw.rectangle(box, fill=background)
            if not char.data.strip():
                continue
            draw.text(box[:2], char.data, font=font, fill=color(char.fg, DEFAULT_FG))
    return image


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", help="captured ANSI text")
    parser.add_argument("output", help="PNG to write")
    parser.add_argument("--cols", type=int, default=120)
    parser.add_argument("--rows", type=int, default=30, help="rows to draw")
    parser.add_argument(
        "--screen-rows",
        type=int,
        default=0,
        help="terminal height of the capture, when longer than --rows (draws the top)",
    )
    parser.add_argument(
        "--screen-cols",
        type=int,
        default=0,
        help="terminal width of the capture, when wider than --cols (draws the left)",
    )
    parser.add_argument("--font", default=r"C:\Windows\Fonts\consola.ttf")
    parser.add_argument("--size", type=int, default=20)
    parser.add_argument("--pad", type=int, default=14)
    args = parser.parse_args()

    raw = open(args.input, encoding="utf-8", errors="replace").read()
    # A bare LF moves down without returning to column 0 in a terminal.
    raw = raw.replace("\r\n", "\n").replace("\n", "\r\n")
    image = render(
        raw,
        args.cols,
        args.rows,
        args.font,
        args.size,
        args.pad,
        args.screen_rows,
        args.screen_cols,
    )
    image.save(args.output)
    print(f"wrote {args.output} {image.size[0]}x{image.size[1]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

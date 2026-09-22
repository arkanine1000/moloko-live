"""Colour maths the tools share: OKLab conversion, hex colours and the palettes the packs are built with.

A palette maps the game's colours to screen colours. Three kinds exist:
  tone list   palettes/NAME.hex, one #rrggbb per line: each game colour becomes the entry nearest in lightness
  lift rule   palettes/NAME.json with "rule" (from the recolour fitter): a curve applied to every colour
  none        the game's own colours
"""
import json

import numpy as np

# OKLab (Björn Ottosson, 2020): linear sRGB -> LMS -> cube root -> Lab.
_M1 = np.array([[0.4122214708, 0.5363325363, 0.0514459929], [0.2119034982, 0.6806995451, 0.1073969566],
                [0.0883024619, 0.2817188376, 0.6299787005]])
_M2 = np.array([[0.2104542553, 0.7936177850, -0.0040720468], [1.9779984951, -2.4285922050, 0.4505937099],
                [0.0259040371, 0.7827717662, -0.8086757660]])
_M1_INV, _M2_INV = np.linalg.inv(_M1), np.linalg.inv(_M2)

# Tone mapping weighs lightness far above hue and chroma: the palettes are near-monochrome grades, so lightness
# decides the entry and hue only breaks ties between entries of the same lightness.
TONE_HUE_WEIGHT = 0.01


def srgb_to_oklab(rgb):
    """(..., 3) sRGB 0-255 -> (..., 3) OKLab."""
    c = np.asarray(rgb, float) / 255.0
    lin = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return np.cbrt(lin @ _M1.T) @ _M2.T


def oklab_to_srgb(lab):
    """(..., 3) OKLab -> (..., 3) uint8 sRGB, clipped to the gamut."""
    lin = np.clip(((lab @ _M2_INV.T) ** 3) @ _M1_INV.T, 0.0, 1.0)
    c = np.where(lin <= 0.0031308, 12.92 * lin, 1.055 * lin ** (1 / 2.4) - 0.055)
    return np.round(c * 255).astype(np.uint8)


def hex_to_rgb(text):
    """'#rrggbb' or 'rrggbb' -> (r, g, b)."""
    text = text.strip().lstrip("#")
    return tuple(int(text[i:i + 2], 16) for i in (0, 2, 4))


def hex_to_rgb_array(texts):
    """A list of hex colours -> (n, 3) uint8."""
    return np.array([hex_to_rgb(t) for t in texts], np.uint8).reshape(-1, 3)


def rgb_to_hex(rgb):
    """(r, g, b) -> '#rrggbb'."""
    return "#" + bytes(int(v) for v in rgb).hex()


def read_hex_palette(path):
    """A .hex file (one colour per line, blank lines ignored) -> (n, 3) uint8, in file order."""
    return hex_to_rgb_array([line for line in path.read_text().splitlines() if line.strip()])


class TonePalette:
    """A list of colours; each game colour maps to the entry nearest in OKLab, lightness weighted over hue."""

    def __init__(self, rgb):
        self.rgb = np.asarray(rgb, np.uint8)
        self.lab = srgb_to_oklab(self.rgb)

    def map(self, rgb):
        """(n, 3) uint8 -> (n, 3) uint8."""
        lab = srgb_to_oklab(rgb)
        d = (lab[:, None, 0] - self.lab[None, :, 0]) ** 2 \
            + TONE_HUE_WEIGHT * ((lab[:, None, 1:] - self.lab[None, :, 1:]) ** 2).sum(-1)
        return self.rgb[d.argmin(1)]


def lift(rgb, rule):
    """(n, 3) uint8 game colours -> (n, 3) uint8 under a lift rule, in OKLab/OKLCH:

        lightness  L' = l0 + (1 - l0) * L^gamma
        chroma     C' = c0 + c1 * C + c2 * L' (1 - L')
        hue        h' = h0 + h1 * L'  (degrees)

    rule = {"l0", "gamma", "chroma": [c0, c1, c2], "hue": [h0, h1]}."""
    lab = srgb_to_oklab(np.asarray(rgb, float).reshape(-1, 3))
    L, C = np.clip(lab[:, 0], 0, 1), np.hypot(lab[:, 1], lab[:, 2])
    l0 = rule["l0"]
    Lp = l0 + (1 - l0) * L ** rule["gamma"]
    c0, c1, c2 = rule["chroma"]
    Cp = np.clip(c0 + c1 * C + c2 * Lp * (1 - Lp), 0, None)
    h = np.radians(rule["hue"][0] + rule["hue"][1] * Lp)
    return oklab_to_srgb(np.column_stack([Lp, Cp * np.cos(h), Cp * np.sin(h)]))


class LiftPalette:
    """A lift rule (see `lift`), the same for every scene."""

    def __init__(self, rule):
        self.rule = rule

    def map(self, rgb):
        return lift(rgb, self.rule)


class Identity:
    """The game's own colours."""

    def map(self, rgb):
        return np.asarray(rgb, np.uint8)


def available_palettes(palettes_dir):
    """Names `load_palette` accepts: 'none' and every .hex or rule .json under palettes_dir."""
    names = ["none"]
    for path in sorted(palettes_dir.iterdir()):
        if path.suffix == ".hex" or (path.suffix == ".json" and "rule" in json.loads(path.read_text())):
            names.append(path.stem)
    return names


def load_palette(name, palettes_dir):
    """A palette by name, looked up under palettes_dir. Raises LookupError for an unknown name."""
    if name == "none":
        return Identity()
    rule = palettes_dir / f"{name}.json"
    if rule.is_file() and "rule" in (data := json.loads(rule.read_text())):
        return LiftPalette(data["rule"])
    tones = palettes_dir / f"{name}.hex"
    if tones.is_file():
        return TonePalette(read_hex_palette(tones))
    raise LookupError(f"unknown palette {name!r}; known: {', '.join(available_palettes(palettes_dir))}")

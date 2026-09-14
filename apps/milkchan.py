#!/usr/bin/env python3
"""Milk-chan sprite viewer. Needs Pillow + pygame-ce (requirements.txt) and a Python with tkinter.

Sprites are assembled from the game's own definitions (rip/raw/sprites *.rpy) and the
ripped layers (rip/frames), so the scripts' odd layer reuses are respected.

  python apps/milkchan.py                  GUI
  python apps/milkchan.py --list           print every pose / emotion / mood
  python apps/milkchan.py --render out.png --pose arms_down --emotion smile --mood 2 [--eyes N] [--talk N] [--scale K]

Keys: P/E/M cycle pose/emotion/mood · C cycle palette (Shift reverses) · Space play/stop idle ·
      Left/Right step frame · T talk · I pixel-perfect scaling · D dialogue · Click/Enter advance · Q/Esc quit

Palettes are applied at runtime. palettes/*.json (tools/recolor_palette.py) map game colours exactly;
palettes/*.hex use tone mapping: each colour maps to the entry nearest in OKLab lightness, hue lightly
weighted. milkchan-neutral comes first after "none"; the fitted neutral grade is last.

Dialogue mode reproduces the game's say screen: textbox, Retro Gaming 31px, 30 cps typewriter,
and speaker_callback's talk logic (mouth flaps + narr.ogg loop while typing, 0.3s fadeout after).
Lines come from apps/milkchan_lines.json (ids only) resolved against rip/dialogue/milkchan_en.jsonl.
"""
import argparse
import json
import os
import random
import re
import sys
import time
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

RIP = Path(__file__).resolve().parent.parent / "rip"
LINES_FILE = Path(__file__).with_name("milkchan_lines.json")
POSES = {"arms_crossed": "sprites arms_crossed.rpy", "arms_down": "sprites arms_down.rpy", "one_arm": "sprites one_arm.rpy"}
PIXEL = 2                   # the art sits on an exact 2x2 grid at 1920x1080
CROP = (76, 44, 978, 1080)  # union bbox of all 232 sprite layers (full-res coordinates)
NATIVE = ((CROP[2] - CROP[0]) // PIXEL, (CROP[3] - CROP[1]) // PIXEL)
BG = "#1e232c"              # firefly-neutral index 0

# Say screen, in the game's 1920x1080 coordinates (gui.rpy / screens.rpy)
SCREEN = (1920, 1080)
SPRITE_OFFSET = (-20, 53)   # default `show`: 1959x1027 LiveComposite at xalign .5 yalign 1.0 -> (-19.5, 53)
TEXTBOX_POS = (722, 704)    # gui/textbox.png at xalign .9 / yalign 1.0 in a 393px window at yalign .95
TEXT_POS, TEXT_WIDTH = (770, 744), 1030
TEXT_SIZE, TEXT_COLOR, LINE_SPACING = 31, "#ac3232", 15
CPS = 30                    # preferences.text_cps
TALK_SFX, TALK_VOLUME, TALK_FADE_MS = "audio/narr.ogg", 0.6, 300
MISSING_GLYPHS = {"…": "...", "—": "-", "–": "-"}  # Retro Gaming has no glyph for these

PALETTES_DIR = Path(__file__).resolve().parent.parent / "palettes"  # tools/palettes.py extract output (*.hex)
# Tone mapping: OKLab distance with hue/chroma down-weighted against lightness. Fitted on cg_firefly ->
# firefly(-neutral).png: pure lightness reproduces them best (mean dE 0.0078; w=0.1 gives 0.0085, w=1 0.0091),
# so hue is only a tie-breaker, for palettes that have several hues at the same lightness.
TONE_HUE_WEIGHT = 0.01
NEUTRAL_GRADE = (0.621, 0.992, 0.0116)  # chroma scale, L scale, L offset (same fit as tools/palettes.py)


@dataclass
class Frame:
    path: str
    hold: tuple  # seconds; several values = ATL `choice:` (pick one at random)


@dataclass
class Sprite:
    name: str
    pose: str
    emotion: str
    mood: str
    layers: list  # ("static", path) | ("eyes", [Frame]) | ("mouth", closed_path, [Frame]), bottom to top

    @property
    def eyes(self):
        return next((l[1] for l in self.layers if l[0] == "eyes"), [])

    @property
    def talk(self):
        return next((l[2] for l in self.layers if l[0] == "mouth"), [])


# ---------------------------------------------------------------- script parsing

IMAGE_BLOCK = re.compile(r"^(\s*)image\s+(\w+)\s*:\s*$")
COMPOSITE = re.compile(r"^\s*image\s+(gg_\w+)\s*=\s*LiveComposite\(")
LAYER = re.compile(r"^\s*\(\s*(-?\d+)\s*,\s*(-?\d+)\s*\)\s*,\s*(.+?),?\s*$")
NAME = re.compile(r"^gg_(\w+?)_(\d+)(?:_ad|_oa)?$")


def parse_atl(lines):
    frames = []
    for line in lines:
        s = line.strip()
        if m := re.fullmatch(r'"([^"]+)"', s):
            frames.append(Frame(m.group(1), ()))
        elif (m := re.fullmatch(r"(?:pause\s+)?([\d.]+)", s)) and frames:
            frames[-1].hold += (float(m.group(1)),)
    return frames


def load_sprites(raw):
    # Ren'Py image names are global and later definitions win, so collect ATL blocks across
    # all sprite files (in Ren'Py's load order) before resolving composites.
    blocks, composites = {}, []
    for pose, fname in sorted(POSES.items(), key=lambda kv: kv[1]):
        lines = (raw / fname).read_text(encoding="utf-8").splitlines()
        i = 0
        while i < len(lines):
            if m := IMAGE_BLOCK.match(lines[i]):
                indent, j = len(m.group(1)), i + 1
                while j < len(lines) and (not lines[j].strip() or len(lines[j]) - len(lines[j].lstrip()) > indent):
                    j += 1
                blocks[m.group(2)] = parse_atl(lines[i + 1:j])
                i = j
            elif m := COMPOSITE.match(lines[i]):
                j = i + 1
                while j < len(lines) and lines[j].strip() != ")":
                    j += 1
                composites.append((pose, m.group(1), lines[i + 1:j]))
                i = j + 1
            else:
                i += 1

    sprites = []
    for pose, name, body in composites:
        layers = []
        for line in body:
            if not (m := LAYER.match(line)):
                continue
            if (m.group(1), m.group(2)) != ("0", "0"):
                print(f"warning: {name} has an offset layer, drawn at 0,0", file=sys.stderr)
            expr = m.group(3)
            strs = re.findall(r'"([^"]+)"', expr)
            if expr.startswith("WhileSpeaking"):
                args = strs[1:]  # drop the speaker tag
                closed = next((s for s in args if s.endswith(".png")), None)
                anim = next((blocks[s] for s in args if s in blocks), [])
                layers.append(("mouth", closed, anim))
            elif strs and strs[0].endswith(".png"):
                layers.append(("static", strs[0]))
            elif strs:
                layers.append(("eyes", blocks.get(strs[0], [])))
        emotion, mood = NAME.match(name).groups()
        sprites.append(Sprite(name, pose, emotion, mood, layers))
    return sprites


# ---------------------------------------------------------------- rendering

class Assets:
    def __init__(self, frames_dir):
        self.roots = [frames_dir / "images", frames_dir]

    def _open(self, rel):
        for root in self.roots:
            if (p := root / rel).is_file():
                return Image.open(p).convert("RGBA")
        print(f"warning: missing image {rel}", file=sys.stderr)
        return None

    @lru_cache(maxsize=512)
    def layer(self, rel):
        im = self._open(rel)
        if im is None:
            return Image.new("RGBA", NATIVE)
        return im.crop(CROP).resize(NATIVE, Image.NEAREST)  # one sample per 2x2 block: lossless

    @lru_cache(maxsize=1)
    def textbox(self):
        im = self._open("gui/textbox.png") or Image.new("RGBA", (2, 2))
        return im.resize((im.width // PIXEL, im.height // PIXEL), Image.NEAREST)


def compose(assets, sprite, eye_i, talk_i):
    """Sprite cropped to CROP at native resolution. talk_i None = mouth closed."""
    out = Image.new("RGBA", NATIVE)
    for kind, *rest in sprite.layers:
        if kind == "static":
            rel = rest[0]
        elif kind == "eyes":
            rel = rest[0][eye_i % len(rest[0])].path if rest[0] else None
        else:
            closed, anim = rest
            rel = anim[talk_i % len(anim)].path if talk_i is not None and anim else closed
        if rel:
            out.alpha_composite(assets.layer(rel))
    return out


def compose_scene(assets, figure):
    """Full say-screen frame at native resolution: sprite at its in-game position, textbox on top."""
    scene = Image.new("RGBA", (SCREEN[0] // PIXEL, SCREEN[1] // PIXEL))
    x, y = (CROP[0] + SPRITE_OFFSET[0]) // PIXEL, (CROP[1] + SPRITE_OFFSET[1]) // PIXEL
    scene.alpha_composite(figure.crop((0, 0, min(figure.width, scene.width - x), min(figure.height, scene.height - y))), (x, y))
    scene.alpha_composite(assets.textbox(), (TEXTBOX_POS[0] // PIXEL, TEXTBOX_POS[1] // PIXEL))
    return scene


def fit(img, box_w, box_h, integer):
    """integer: largest whole multiple that fits (pixel-perfect).
    Otherwise sharp fill: nearest-neighbour up to the next whole multiple, then area-average
    down to the exact fit, so pixels stay crisp with at most a 1px soft edge."""
    f = min(box_w / img.width, box_h / img.height)
    if integer and f >= 1:
        k = int(f)
        return img.resize((img.width * k, img.height * k), Image.NEAREST), f"×{k}"
    size = (max(1, round(img.width * f)), max(1, round(img.height * f)))
    if f >= 1:
        k = -(-size[0] // img.width)
        img = img.resize((img.width * k, img.height * k), Image.NEAREST)
    return img.resize(size, Image.BOX), f"×{f:.2f}"


@lru_cache(maxsize=16)
def load_font(path, size):
    return ImageFont.truetype(str(path), size)


def wrap(text, font, width):
    """Greedy word wrap at full-res metrics, so line breaks don't change with window size."""
    lines = []
    for para in text.split("\n"):
        line = ""
        for word in para.split(" "):
            trial = f"{line} {word}" if line else word
            if line and font.getlength(trial) > width:
                lines.append(line)
                line = word
            else:
                line = trial
        lines.append(line)
    return lines


# ---------------------------------------------------------------- palettes

_M1 = np.array([[0.4122214708, 0.5363325363, 0.0514459929], [0.2119034982, 0.6806995451, 0.1073969566],
                [0.0883024619, 0.2817188376, 0.6299787005]])
_M2 = np.array([[0.2104542553, 0.7936177850, -0.0040720468], [1.9779984951, -2.4285922050, 0.4505937099],
                [0.0259040371, 0.7827717662, -0.8086757660]])
_M1_INV, _M2_INV = np.linalg.inv(_M1), np.linalg.inv(_M2)


def srgb_to_oklab(rgb):
    c = np.asarray(rgb, float) / 255.0
    lin = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return np.cbrt(lin @ _M1.T) @ _M2.T


def oklab_to_srgb(lab):
    lin = np.clip(((lab @ _M2_INV.T) ** 3) @ _M1_INV.T, 0.0, 1.0)
    c = np.where(lin <= 0.0031308, 12.92 * lin, 1.055 * lin ** (1 / 2.4) - 0.055)
    return np.round(c * 255).astype(np.uint8)


class Palette:
    """Runtime recolour of rendered frames.

    With colours: tone mapping. Each colour becomes the palette entry nearest in OKLab, with
    lightness weighted over hue. That is how the user's firefly.png relates to cg_firefly: lightness
    kept, hue swapped for the palette's. With grade=True: the fitted "neutral" grade (chroma x0.621)."""

    def __init__(self, name, colors=None, grade=False, mapping=None):
        self.name, self.grade, self.ground, self.map_keys = name, grade, BG, None
        if mapping:  # exact game colour -> colour (tools/recolor_palette.py); tone mapping covers the rest
            keys = np.array([int(k.lstrip("#"), 16) for k in mapping], np.uint32)
            vals = np.array([[int(v.lstrip("#")[i:i + 2], 16) for i in (0, 2, 4)] for v in mapping.values()], np.uint8)
            order = keys.argsort()
            self.map_keys, self.map_vals = keys[order], vals[order]
            colors = colors or sorted({v.lstrip("#") for v in mapping.values()})
        if colors:
            self.rgb = np.array([[int(c[i:i + 2], 16) for i in (0, 2, 4)] for c in colors], np.uint8)
            self.lab = srgb_to_oklab(self.rgb)
            # The game's near-black sweater, hair and textbox fill all land on the palette's darkest
            # entry; paint the canvas a step darker so the silhouette doesn't dissolve into it.
            darkest = self.lab[self.lab[:, 0].argmin()].copy()
            darkest[0] = max(0.0, darkest[0] - 0.07)
            self.ground = "#" + bytes(oklab_to_srgb(darkest[None])[0].tolist()).hex()

    def map(self, rgb):
        """(n, 3) uint8 -> (n, 3) uint8"""
        lab = srgb_to_oklab(rgb)
        if self.grade:
            chroma, l_scale, l_offset = NEUTRAL_GRADE
            lab[:, 0] = l_scale * lab[:, 0] + l_offset
            lab[:, 1:] *= chroma
            return oklab_to_srgb(lab)
        d = (lab[:, None, 0] - self.lab[None, :, 0]) ** 2 \
            + TONE_HUE_WEIGHT * ((lab[:, None, 1:] - self.lab[None, :, 1:]) ** 2).sum(-1)
        out = self.rgb[d.argmin(1)]
        if self.map_keys is not None:
            key = (rgb[:, 0].astype(np.uint32) << 16) | (rgb[:, 1].astype(np.uint32) << 8) | rgb[:, 2]
            i = np.searchsorted(self.map_keys, key).clip(0, len(self.map_keys) - 1)
            hit = self.map_keys[i] == key
            out[hit] = self.map_vals[i[hit]]
        return out

    def apply(self, img):
        a = np.array(img)  # RGBA copy; alpha is kept as-is
        visible = a[..., 3] > 0
        px = a[visible][:, :3].astype(np.uint32)
        uniq, inv = np.unique((px[:, 0] << 16) | (px[:, 1] << 8) | px[:, 2], return_inverse=True)
        colours = np.stack([(uniq >> 16) & 255, (uniq >> 8) & 255, uniq & 255], 1).astype(np.uint8)
        a[visible, :3] = self.map(colours)[inv.ravel()]  # map only the few hundred distinct colours
        return Image.fromarray(a, "RGBA")

    def hex(self, color):
        rgb = np.array([[int(color[i:i + 2], 16) for i in (1, 3, 5)]], np.uint8)
        return "#" + bytes(self.map(rgb)[0].tolist()).hex()


PREFERRED_PALETTES = ("milkchan-neutral", "firefly-neutral")  # next in line after "none"


def load_palettes():
    """"none", the preferred palettes, the rest of palettes/ (*.json maps, *.hex lists), then the neutral grade."""
    pals = {"none": None}
    files = sorted([*PALETTES_DIR.glob("*.json"), *PALETTES_DIR.glob("*.hex")],
                   key=lambda f: (PREFERRED_PALETTES.index(f.stem) if f.stem in PREFERRED_PALETTES else len(PREFERRED_PALETTES), f.stem))
    for f in files:
        if f.suffix == ".json":
            if "map" in (data := json.loads(f.read_text())):  # other .json files are palettes.py extract metadata
                pals[f.stem] = Palette(f.stem, mapping=data["map"])
                pals[f.stem].ground = data.get("ground", pals[f.stem].ground)
            continue
        colors = [l.strip().lstrip("#") for l in f.read_text().splitlines() if l.strip()]
        if colors:
            pals[f.stem] = Palette(f.stem, colors)
    pals["neutral grade"] = Palette("neutral grade", grade=True)
    return pals


# ---------------------------------------------------------------- dialogue

def load_dialogue(rip):
    rows = {}
    with open(rip / "dialogue" / "milkchan_en.jsonl", encoding="utf-8") as fh:
        for line in fh:
            r = json.loads(line)
            rows.setdefault(r["id"], r)
    picks = json.loads(LINES_FILE.read_text(encoding="utf-8"))["lines"]
    missing = [p for p in picks if p not in rows]
    if missing:
        print(f"warning: {len(missing)} picked lines not found: {missing[:3]}", file=sys.stderr)
    return [rows[p] for p in picks if p in rows]


class TalkSound:
    """speaker_callback: loop narr.ogg on "show", fade out 0.3s on "slow_done"/"end"."""

    def __init__(self, path):
        self.sound = self.channel = None
        try:
            os.environ.setdefault("PYGAME_HIDE_SUPPORT_PROMPT", "1")
            import pygame
            pygame.mixer.init()
            self.sound = pygame.mixer.Sound(str(path))
            self.sound.set_volume(TALK_VOLUME)
        except Exception as e:  # no audio device, missing file, no pygame: stay silent
            print(f"warning: talk sound disabled ({e})", file=sys.stderr)

    def start(self):
        if self.sound:
            self.sound.stop()  # renpy.music.play restarts the file on every line
            self.channel = self.sound.play(loops=-1)

    def stop(self):
        if self.channel:
            self.channel.fadeout(TALK_FADE_MS)


# ---------------------------------------------------------------- GUI

class App:
    def __init__(self, sprites, assets, rip=RIP):
        import tkinter as tk
        from tkinter import ttk
        from PIL import ImageTk

        self.tk, self.ImageTk, self.assets, self.rip = tk, ImageTk, assets, rip
        self.index = {}
        for s in sprites:
            self.index.setdefault(s.pose, {}).setdefault(s.emotion, {})[s.mood] = s

        self.root = root = tk.Tk()
        root.title("Milk-chan — P/E/M pose/emotion/mood · C palette (Shift: back) · Space play/stop · ←/→ step · T talk · "
                   "I pixel-perfect · D dialogue (click/Enter advance) · Q quit")
        root.geometry("720x860")
        root.configure(bg=BG)

        bar = ttk.Frame(root, padding=(6, 6, 6, 2))  # selection row
        bar.pack(fill="x")
        bar2 = ttk.Frame(root, padding=(6, 0, 6, 6))  # toggles row
        bar2.pack(fill="x")
        self.pose, self.emotion, self.mood = tk.StringVar(value="arms_down"), tk.StringVar(value="neutral"), tk.StringVar(value="1")
        self.combos = {}
        for label, var in (("Pose", self.pose), ("Emotion", self.emotion), ("Mood", self.mood)):
            ttk.Label(bar, text=label).pack(side="left", padx=(6, 2))
            cb = ttk.Combobox(bar, textvariable=var, state="readonly", width=12 if label != "Mood" else 3)
            cb.pack(side="left")
            cb.bind("<<ComboboxSelected>>", self.on_select)
            self.combos[label] = cb
        self.palettes, self.text_colors, self.frames = load_palettes(), {}, {}  # frames: native frame cache
        self.palette = tk.StringVar(value="none")
        ttk.Label(bar2, text="Palette").pack(side="left", padx=(6, 2))
        pc = ttk.Combobox(bar2, textvariable=self.palette, state="readonly", width=16, values=list(self.palettes))
        pc.pack(side="left")
        pc.bind("<<ComboboxSelected>>", lambda e: (self.canvas.focus_set(), self.redraw()))
        self.play_btn = ttk.Button(bar2, text="Stop", width=6, command=self.toggle_play, takefocus=False)
        self.play_btn.pack(side="left", padx=(12, 2))
        self.talking = tk.BooleanVar(value=False)
        ttk.Checkbutton(bar2, text="Talk", variable=self.talking, command=self.on_talk, takefocus=False).pack(side="left", padx=4)
        self.integer = tk.BooleanVar(value=False)
        ttk.Checkbutton(bar2, text="Pixel-perfect", variable=self.integer, command=self.redraw, takefocus=False).pack(side="left", padx=4)
        self.dialogue = tk.BooleanVar(value=False)
        ttk.Checkbutton(bar2, text="Dialogue", variable=self.dialogue, command=self.on_dialogue, takefocus=False).pack(side="left", padx=4)

        self.canvas = tk.Canvas(root, bg=BG, highlightthickness=0)
        self.canvas.pack(fill="both", expand=True)
        self.status = ttk.Label(root, padding=(8, 3), text="")
        self.status.pack(fill="x")

        self.playing, self.eye_i, self.talk_i = True, 0, 0
        self.timers, self.photo, self.text_photo, self.resize_job = {}, None, None, None
        self.view = (0, 0, 1.0)  # scene origin on canvas + full-res -> display factor
        # dialogue state
        self.lines, self.sound, self.line, self.order, self.pos = None, None, None, [], 0
        self.wrapped, self.total, self.chars, self.typing, self.type_start = [], 0, 0, False, 0.0
        self.font_path = rip / "raw" / "images" / "122.ttf"

        self.canvas.bind("<Configure>", self.on_resize)
        self.canvas.bind("<Button-1>", lambda e: self.advance())
        for keys, fn in ((("<space>",), self.toggle_play), (("<Left>",), lambda: self.step(-1)), (("<Right>",), lambda: self.step(1)),
                         (("t", "T"), self.flip_talk), (("i", "I"), self.flip_integer), (("q", "Q", "<Escape>"), root.destroy),
                         (("p",), lambda: self.cycle("pose", 1)), (("P",), lambda: self.cycle("pose", -1)),
                         (("e",), lambda: self.cycle("emotion", 1)), (("E",), lambda: self.cycle("emotion", -1)),
                         (("m",), lambda: self.cycle("mood", 1)), (("M",), lambda: self.cycle("mood", -1)),
                         (("d", "D"), self.flip_dialogue), (("<Return>", "<KP_Enter>"), self.advance),
                         (("c",), lambda: self.cycle_palette(1)), (("C",), lambda: self.cycle_palette(-1))):
            for k in keys:
                root.bind(k, lambda e, fn=fn: (fn(), "break")[1])
        self.refresh_choices()

    # selection ---------------------------------------------------------
    def refresh_choices(self):
        poses = list(self.index)
        emotions = list(self.index[self.pose.get()])
        if self.emotion.get() not in emotions:
            self.emotion.set(emotions[0])
        moods = sorted(self.index[self.pose.get()][self.emotion.get()], key=int)
        if self.mood.get() not in moods:
            self.mood.set(moods[0])
        for label, values in (("Pose", poses), ("Emotion", emotions), ("Mood", moods)):
            self.combos[label]["values"] = values
        self.sprite = self.index[self.pose.get()][self.emotion.get()][self.mood.get()]
        self.restart()

    def cycle(self, which, d):
        pose, emotion = self.pose.get(), self.emotion.get()
        var, values = {
            "pose": (self.pose, list(self.index)),
            "emotion": (self.emotion, list(self.index[pose])),
            "mood": (self.mood, sorted(self.index[pose][emotion], key=int)),
        }[which]
        var.set(values[(values.index(var.get()) + d) % len(values)])
        self.refresh_choices()

    def on_select(self, _event):
        self.canvas.focus_set()  # keep Space/arrows for the viewer, not the combobox
        self.refresh_choices()

    # playback ----------------------------------------------------------
    def mouth_active(self):
        # In dialogue mode the mouth follows the typewriter, like WhileSpeaking in the game.
        return self.typing if self.dialogue.get() else self.talking.get()

    def cancel(self, which):
        if job := self.timers.pop(which, None):
            self.root.after_cancel(job)

    def schedule(self, which):
        self.cancel(which)
        frames = self.sprite.eyes if which == "eyes" else self.sprite.talk
        if not frames or (which == "eyes" and not self.playing) or (which == "talk" and not self.mouth_active()):
            return
        i = self.eye_i if which == "eyes" else self.talk_i
        hold = random.choice(frames[i % len(frames)].hold or (0.1,))
        self.timers[which] = self.root.after(int(hold * 1000), lambda: self.tick(which))

    def tick(self, which):
        if which == "eyes":
            self.eye_i = (self.eye_i + 1) % len(self.sprite.eyes)
        else:
            self.talk_i = (self.talk_i + 1) % len(self.sprite.talk)
        self.redraw()
        self.schedule(which)

    def restart(self):
        self.schedule("eyes")
        self.schedule("talk")
        self.redraw()

    def toggle_play(self):
        self.playing = not self.playing
        self.play_btn.configure(text="Stop" if self.playing else "Play")
        if not self.playing:
            self.cancel("eyes")
        self.restart()

    def step(self, d):
        if self.playing:
            self.toggle_play()
        if self.sprite.eyes:
            self.eye_i = (self.eye_i + d) % len(self.sprite.eyes)
        if self.mouth_active() and self.sprite.talk:
            self.talk_i = (self.talk_i + d) % len(self.sprite.talk)
        self.redraw()

    def on_talk(self):
        self.schedule("talk")
        self.redraw()

    def flip_talk(self):
        self.talking.set(not self.talking.get())
        self.on_talk()

    def flip_integer(self):
        self.integer.set(not self.integer.get())
        self.redraw()

    # palettes ----------------------------------------------------------
    def current_palette(self):
        return self.palettes.get(self.palette.get())

    def cycle_palette(self, d):
        names = list(self.palettes)
        self.palette.set(names[(names.index(self.palette.get()) + d) % len(names)])
        self.redraw()

    def text_color(self):
        pal = self.current_palette()
        if pal is None:
            return TEXT_COLOR
        if pal.name not in self.text_colors:
            self.text_colors[pal.name] = pal.hex(TEXT_COLOR)
        return self.text_colors[pal.name]

    # dialogue ----------------------------------------------------------
    def flip_dialogue(self):
        self.dialogue.set(not self.dialogue.get())
        self.on_dialogue()

    def on_dialogue(self):
        if not self.dialogue.get():
            self.cancel("type")
            self.typing, self.line = False, None
            if self.sound:
                self.sound.stop()
            self.restart()
            return
        if self.lines is None:
            try:
                self.lines = load_dialogue(self.rip)
            except (OSError, ValueError, KeyError) as e:
                self.lines = []
                print(f"warning: dialogue unavailable ({e})", file=sys.stderr)
        if not self.lines:
            self.dialogue.set(False)
            self.status.configure(text="No dialogue: run tools/dialogue.py and check apps/milkchan_lines.json")
            return
        if self.sound is None:
            self.sound = TalkSound(self.rip / "raw" / TALK_SFX)
        self.reshuffle()
        self.start_line()

    def reshuffle(self, avoid=None):
        self.order = list(range(len(self.lines)))
        random.shuffle(self.order)
        if avoid is not None and len(self.order) > 1 and self.order[0] == avoid:
            self.order[0], self.order[1] = self.order[1], self.order[0]
        self.pos = 0

    def start_line(self):
        self.line = row = self.lines[self.order[self.pos]]
        if row.get("pose") in self.index and row.get("emotion") in self.index[row["pose"]] \
                and row.get("mood") in self.index[row["pose"]][row["emotion"]]:
            self.pose.set(row["pose"]), self.emotion.set(row["emotion"]), self.mood.set(row["mood"])
        text = row["text"]
        for bad, good in MISSING_GLYPHS.items():
            text = text.replace(bad, good)
        self.wrapped = wrap(text, load_font(self.font_path, TEXT_SIZE), TEXT_WIDTH)
        self.total, self.chars, self.talk_i = sum(map(len, self.wrapped)), 0, 0
        self.typing, self.type_start = True, time.monotonic()
        self.sound.start()
        self.refresh_choices()  # picks up the sprite and arms the talk timer (typing is on)
        self.type_tick()

    def type_tick(self):
        self.cancel("type")
        if not self.typing:
            return
        chars = min(self.total, int((time.monotonic() - self.type_start) * CPS))
        if chars != self.chars:
            self.chars = chars
            self.redraw_text()
        if chars >= self.total:
            self.finish_typing()
        else:
            self.timers["type"] = self.root.after(15, self.type_tick)

    def finish_typing(self):
        """slow_done: reveal the rest, close the mouth, fade the talk loop."""
        self.cancel("type")
        self.cancel("talk")
        self.typing, self.chars = False, self.total
        self.sound.stop()
        self.redraw()

    def advance(self):
        if not self.dialogue.get():
            return
        if self.typing:  # first click completes the line, like Ren'Py
            self.finish_typing()
            return
        self.pos += 1
        if self.pos >= len(self.order):
            self.reshuffle(avoid=self.order[-1])
        self.start_line()

    # drawing -----------------------------------------------------------
    def on_resize(self, _event):
        if self.resize_job:
            self.root.after_cancel(self.resize_job)
        self.resize_job = self.root.after(30, self.redraw)

    def redraw(self):
        w, h = max(1, self.canvas.winfo_width()), max(1, self.canvas.winfo_height())
        s = self.sprite
        eye_i = self.eye_i % len(s.eyes) if s.eyes else 0
        talk_i = self.talk_i % len(s.talk) if self.mouth_active() and s.talk else None
        pal, dialogue = self.current_palette(), self.dialogue.get()
        ground = pal.ground if pal else BG
        if self.canvas.cget("bg") != ground:
            self.canvas.configure(bg=ground)
            self.root.configure(bg=ground)
        # Blinks and mouth flaps cycle through a handful of frames, so cache each finished native
        # frame; the palette remap (20-35 ms) then runs once per distinct frame, not per tick.
        key = (s.name, eye_i, talk_i, self.palette.get(), dialogue)
        if (native := self.frames.get(key)) is None:
            native = compose(self.assets, s, eye_i, talk_i)
            if dialogue:
                native = compose_scene(self.assets, native)
            if pal:
                native = pal.apply(native)  # at native resolution, before scaling blends colours
            if len(self.frames) >= 64:
                self.frames.pop(next(iter(self.frames)))
            self.frames[key] = native
        img, scale = fit(native, w, h, self.integer.get())
        origin = ((w - img.width) // 2, (h - img.height) // 2) if dialogue else ((w - img.width) // 2, h - img.height)
        self.view = (*origin, img.width / SCREEN[0])
        self.photo = self.ImageTk.PhotoImage(img)
        self.canvas.delete("scene")
        self.canvas.create_image(*origin, image=self.photo, anchor="nw", tags="scene")
        self.redraw_text()

        def label(frames, i):
            if not frames:
                return "—"
            return f"{i % len(frames) + 1}/{len(frames)} {Path(frames[i % len(frames)].path).stem.split('_', 1)[-1]}"
        mouth = label(s.talk, self.talk_i) if talk_i is not None else "closed"
        extra = f" · line {self.pos + 1}/{len(self.order)} {self.line['id']}" if self.dialogue.get() and self.line else ""
        palette = f" · {self.palette.get()}" if pal else ""
        self.status.configure(text=f"{s.name} · eyes {label(s.eyes, eye_i)} · mouth {mouth} · {scale} · "
                                   f"{'playing' if self.playing else 'stopped'}{palette}{extra}")

    def redraw_text(self):
        self.canvas.delete("text")
        if not (self.dialogue.get() and self.line and self.chars):
            return
        ox, oy, k = self.view
        full = load_font(self.font_path, TEXT_SIZE)
        line_h = sum(full.getmetrics()) + LINE_SPACING
        font = load_font(self.font_path, max(6, round(TEXT_SIZE * k)))
        img = Image.new("RGBA", (round((TEXT_WIDTH + TEXT_SIZE) * k) + 1, round(len(self.wrapped) * line_h * k) + 1))
        draw, left = ImageDraw.Draw(img), self.chars
        for i, text in enumerate(self.wrapped):
            if left <= 0:
                break
            draw.text((0, round(i * line_h * k)), text[:left], font=font, fill=self.text_color())
            left -= len(text)
        self.text_photo = self.ImageTk.PhotoImage(img)
        self.canvas.create_image(ox + round(TEXT_POS[0] * k), oy + round(TEXT_POS[1] * k), image=self.text_photo,
                                 anchor="nw", tags="text")

    def run(self):
        self.root.mainloop()


# ---------------------------------------------------------------- CLI

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rip", type=Path, default=RIP, help="rip directory (default: %(default)s)")
    ap.add_argument("--list", action="store_true", help="list sprites and exit")
    ap.add_argument("--render", type=Path, metavar="OUT", help="render one frame to a PNG and exit")
    ap.add_argument("--pose", default="arms_down", choices=POSES)
    ap.add_argument("--emotion", default="neutral")
    ap.add_argument("--mood", default="1")
    ap.add_argument("--eyes", type=int, default=0, help="eye frame index")
    ap.add_argument("--talk", type=int, default=None, help="talk frame index (omit for closed mouth)")
    ap.add_argument("--scale", type=int, default=1, help="integer upscale for --render")
    args = ap.parse_args()

    sprites = load_sprites(args.rip / "raw")
    assets = Assets(args.rip / "frames")

    if args.list:
        for s in sprites:
            print(f"{s.pose:13} {s.emotion:8} {s.mood}  {s.name:18} eyes={len(s.eyes)} talk={len(s.talk)} layers={len(s.layers)}")
        return
    if args.render:
        match = [s for s in sprites if (s.pose, s.emotion, s.mood) == (args.pose, args.emotion, args.mood)]
        if not match:
            sys.exit(f"no sprite {args.pose}/{args.emotion}/{args.mood} (see --list)")
        img = compose(assets, match[0], args.eyes, args.talk)
        img.resize((img.width * args.scale, img.height * args.scale), Image.NEAREST).save(args.render)
        print(f"{args.render}: {match[0].name} {img.width * args.scale}x{img.height * args.scale}")
        return
    App(sprites, assets, args.rip).run()


if __name__ == "__main__":
    main()

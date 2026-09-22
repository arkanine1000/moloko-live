#!/usr/bin/env python3
"""Build scene packs for the wallpaper engine (engine/).

  molokolive-pack                                        # every scene
  molokolive-pack mini_cg_run cg_floor --palette firefly-neutral

SCENES lists each scene as the layer stack rip/raw/script.rpy shows. The images themselves (plain paths,
Animation(...), ATL blocks with choice/pause/repeat, LiveComposite) are parsed from rip/raw/art.rpy.

Writes <packs>/<scene>/ (default packs: ~/.local/share/molokolive/packs, outside the repository; rebuilt from scratch
each run):
  manifest.json          format below
  images/<hash>.png      8-bit indexed, index 0 transparent, PLTE = game colours (viewable), deduplicated
  luts/<palette>.bin     256 x BGRA: index -> screen colour (index 0 unused). Palettes are tone lists (*.hex) or
                         lift rules (*.json with "rule", from molokolive-recolour); see colour.py
  preview/<palette>.png  native canvas with the first image of every layer

manifest.json, version 1:
  size [w, h]              native canvas; the game draws everything at x2. mini_cg scenes are cropped to their window.
  crop [x0, y0, x1, y1]    where the canvas sits in the game's native 960x540
  background               index shown where every layer is transparent, and around the canvas
  colours                  "#rrggbb" for index 1, 2, ...
  palettes                 {name: LUT path}
  layers                   bottom to top, each either
    {"name", "choices": [image], "sources": [game path], "drift": drift (optional)}
                                   one image picked at start (a sky pool; a static image is one choice)
  drift                    {"period": s, "steps": [[t, dx, dy], ...]}: the layer's offset in native pixels from time t of
                           each period on, from the script's `circle`/`circle2` transforms
    {"name", "base": image, "steps": [{"hold": [s, ...], "patch": patch | null}], "loop": i | null, "wrap": patch | null}
                                   step 0 shows base; entering step i > 0 applies its patch. Each visit holds for
                                   one of the listed durations, picked at random. After the last step, "wrap"
                                   (last image -> step `loop`) is applied and playback continues at `loop`;
                                   loop null stops on the last step.
  patch                    {"rect": [x0, y0, x1, y1], "image": path, "dirty": [[x0, y0, x1, y1], ...]}: the layer's
                           new pixels inside rect (the bounding box of what changed), and the changed pixels as
                           rectangles on a TILE grid, so the engine knows every dirty region in advance
"""
import argparse
import hashlib
import json
import math
import re
import shutil
import sys
import textwrap
from pathlib import Path

import numpy as np
from PIL import Image

from molokolive.colour import load_palette
from molokolive.paths import PALETTES, RIP, default_packs

NATIVE = (960, 540)
PIXEL = 2
BACKGROUND = (13, 13, 20)  # #0d0d14, the game's near-black and the mini_cg backdrop

# `show skyN at circle: truecenter zoom 1.01`: the zoom hides the edges while `circle` drifts the sky +-2 px.
SKY_ZOOM = 1.01
# The window every mini_cg is drawn in (native px, union over all their frames); the rest is BACKGROUND.
MINI_CROP = (224, 50, 736, 334)
# Dirty-region grid (native px): a patch lists its changed TILE x TILE cells, merged into rectangles.
TILE = 16
# Share of full-resolution pixels allowed off the 2x2 grid before an image is rejected (cg_mirror_gg/57 is 3.9%).
MAX_OFF_GRID = 0.05


def sky(pool):
    """`show skyN at circle: truecenter zoom 1.01`."""
    return {"image": pool, "zoom": SKY_ZOOM, "name": "sky", "drift": "circle"}


def reflection(pool):
    """A cg_mirror_gg pool: `show cg_mirror_ggN at circle2: truecenter zoom 1.01`, between the sky and the mirror."""
    return {"image": pool, "zoom": SKY_ZOOM, "name": "reflection", "drift": "circle2"}


# A layer is an image name, {"image", "zoom", "name"}, or a list of animations: all but the last play once, then
# the last loops.
SCENES = {
    "cg_ceiling": {"layers": [sky("sky3"), "cg_ceiling", "fireflies_anim"]},  # the script adds the fireflies after a choice
    "cg_dream": {"layers": [sky("sky3"), "cg_dream_s"]},  # cg_dream_s is the variant with blinking eyes
    # Never shown by the script (only hidden, next to sky2). Its irises are holes, so it needs a sky behind it.
    "cg_eyelash": {"layers": [sky("sky2"), "cg_eyelash"]},
    "cg_fall_close": {"layers": [sky("sky4"), "cg_fall_close"]},
    "cg_fall_far": {"layers": [sky("sky1"), "cg_fall_far"]},
    "cg_firefly": {"layers": [sky("sky1"), "cg_firefly"]},
    "cg_floor": {"layers": [sky("sky1"), "cg_floor"]},
    "cg_mirror": {"layers": [sky("sky1"), reflection("cg_mirror_gg4"), "cg_mirror_idle"]},
    "cg_mirror_brush": {"layers": [sky("sky1"), reflection("cg_mirror_gg1"), "cg_mirror_brush"]},
    "cg_pills": {"layers": [sky("sky2"), "cg_pills"]},
    "mini_cg_1": {"crop": MINI_CROP, "layers": [["mini_cg_1_1", "mini_cg_1_2"]]},
    "mini_cg_door": {"crop": MINI_CROP, "layers": ["mini_cg_door"]},
    "mini_cg_eyes": {"crop": MINI_CROP, "layers": ["eyes_fear"]},
    "mini_cg_momp": {"crop": MINI_CROP, "layers": ["mini_cg_momp_22"]},
    "mini_cg_run": {"crop": MINI_CROP, "layers": ["mini_cg_run"]},
}

STRING = re.compile(r'"([^"]+)"')


class HelpFormatter(argparse.RawDescriptionHelpFormatter):
    """Help at 100 columns that never breaks a word at a hyphen (palette and file names stay whole)."""

    def __init__(self, prog):
        super().__init__(prog, width=100, max_help_position=30)

    def _split_lines(self, text, width):
        return textwrap.wrap(" ".join(text.split()), width, break_on_hyphens=False)
NUMBER = re.compile(r"(?:pause\s+)?(\d*\.?\d+)")


# ---------------------------------------------------------------- art.rpy

def parse_images(path):
    """Ren'Py image name -> ("static", path) | ("pool", [path]) | ("anim", [(path, holds)], loop) | ("composite", [expr])."""
    lines = path.read_text(encoding="utf-8").splitlines()
    images, i = {}, 0
    while i < len(lines):
        line = lines[i]
        if m := re.match(r'\s*image\s+(\w+)\s*=\s*"([^"]+)"\s*$', line):
            images[m[1]] = ("static", m[2])
            i += 1
        elif m := re.match(r"\s*image\s+(\w+)\s*=\s*(Animation|LiveComposite)\s*\(", line):
            body, depth = "", 0
            while True:
                body += lines[i] + "\n"
                depth += lines[i].count("(") - lines[i].count(")")
                i += 1
                if depth <= 0:
                    break
            body = body[body.index("(") + 1:]
            if m[2] == "Animation":  # loops by default
                images[m[1]] = ("anim", [(p, (float(t),)) for p, t in re.findall(r'"([^"]+)"\s*,\s*(\d*\.?\d+)', body)], 0)
            else:
                layers = []
                for x, y, expr in re.findall(r'\(\s*(-?\d+)\s*,\s*(-?\d+)\s*\)\s*,\s*(WhileSpeaking\([^)]*\)|"[^"]+")', body):
                    if (x, y) != ("0", "0"):
                        sys.exit(f"{m[1]}: offset LiveComposite layers are not supported")
                    # WhileSpeaking(who, talking, silent=Null()): nobody speaks on a wallpaper, so the silent image, which
                    # is nothing when left out (cg_mirror_brush: its mouth and toothbrush are painted into the base).
                    strings = STRING.findall(expr)
                    if expr.startswith("WhileSpeaking"):
                        if len(strings) >= 3:
                            layers.append(strings[2])
                    else:
                        layers.append(strings[0])
                images[m[1]] = ("composite", layers)
        elif m := re.match(r"(\s*)image\s+(\w+)\s*:\s*$", line):
            indent, j = len(m[1]), i + 1
            while j < len(lines) and (not lines[j].strip() or len(lines[j]) - len(lines[j].lstrip()) > indent):
                j += 1
            images[m[2]] = parse_atl(m[2], lines[i + 1:j])
            i = j
        else:
            i += 1
    return images


def parse_atl(name, lines):
    """ATL block of stills, pauses, `choice:` and `repeat`. Choice blocks of stills are a pool (one picked per show);
    choice blocks of numbers are alternative pauses for the preceding still."""
    rows = [(len(l) - len(l.lstrip()), l.strip()) for l in lines if l.strip() and not l.strip().startswith("#")]
    steps, pool, choices, loop, k = [], [], [], None, 0

    def add_hold(extra):
        if not steps:
            sys.exit(f"{name}: pause before the first image")
        steps[-1][1] = tuple(round(h + e, 4) for h in steps[-1][1] for e in extra)

    while k < len(rows):
        indent, text = rows[k]
        k += 1
        if text == "choice:":
            sub = []
            while k < len(rows) and rows[k][0] > indent:
                sub.append(rows[k][1])
                k += 1
            if paths := [p for t in sub for p in STRING.findall(t)]:
                pool += paths
            else:
                choices.append(sum(float(NUMBER.fullmatch(t)[1]) for t in sub))
            continue
        if choices:
            add_hold(choices)
            choices = []
        if m := re.fullmatch(r'"([^"]+)"', text):
            steps.append([m[1], (0.0,)])
        elif m := NUMBER.fullmatch(text):
            add_hold((float(m[1]),))
        elif text == "repeat":
            loop = 0
        else:
            sys.exit(f"{name}: unsupported ATL {text!r}")
    if choices:
        add_hold(choices)
    if pool and not steps:
        return ("pool", pool)
    return ("anim", [(p, h) for p, h in steps], loop)


def parse_transform(name):
    """`transform NAME:` in script.rpy, a repeating block of `ease|linear D xoffset|yoffset V` lines -> {"period": s,
    "steps": [[t, dx, dy], ...]}: the repeating cycle's whole native-pixel offsets and when each takes effect. An offset
    changes where the tweened full-resolution offset crosses half a native pixel (Ren'Py's ease is 0.5 - cos(pi t) / 2),
    so a tween over two native pixels steps twice. The first pass starts from rest; the cycle is the steady state after
    it, and a layer starts at the offset the cycle ends on (the last step's)."""
    lines = (RIP / "raw" / "script.rpy").read_text(encoding="utf-8").splitlines()
    start = next((i for i, l in enumerate(lines) if re.match(rf"\s*transform\s+{name}\s*:\s*$", l)), None)
    if start is None:
        sys.exit(f"transform {name} not found in script.rpy")
    indent = len(lines[start]) - len(lines[start].lstrip())
    tweens, repeat = [], False
    for line in lines[start + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            break
        text = line.strip()
        if not text:
            continue
        if text == "repeat":
            repeat = True
            break
        m = re.fullmatch(r"(ease|linear)\s+([\d.]+)\s+(x|y)offset\s+(-?[\d.]+)", text)
        if not m:
            sys.exit(f"transform {name}: unsupported {text!r}")
        tweens.append((m[1], float(m[2]), m[3], float(m[4]) / PIXEL))
    if not repeat or not tweens:
        sys.exit(f"transform {name}: expected a repeating drift")

    def run(position):
        position, t, steps = dict(position), 0.0, []
        native = {axis: round(v) for axis, v in position.items()}
        for kind, duration, axis, target in tweens:
            a, b = position[axis], target
            if a != b:
                lo, hi = min(a, b), max(a, b)
                edges = [k + 0.5 for k in range(math.floor(lo) - 1, math.ceil(hi) + 1) if lo < k + 0.5 < hi]
                for edge in sorted(edges, reverse=b < a):
                    f = (edge - a) / (b - a)
                    fraction = math.acos(1 - 2 * f) / math.pi if kind == "ease" else f
                    native[axis] = round(edge + 0.5) if b > a else round(edge - 0.5)
                    steps.append([round(t + fraction * duration, 3), native["x"], native["y"]])
            position[axis] = b
            t += duration
        return t, position, steps

    period, rest, _ = run({"x": 0.0, "y": 0.0})
    _, end, steps = run(rest)
    if end != rest or not steps:
        sys.exit(f"transform {name}: its drift doesn't settle into a cycle after one pass")
    return {"period": round(period, 3), "steps": steps}


def resolve(images, spec):
    """Scene layer spec -> [(name, zoom, "choices", [path]) | (name, zoom, "anim", ([(path, holds)], loop))]."""
    if isinstance(spec, list):
        steps, loop = [], None
        for item in spec:
            kind, anim, anim_loop = images[item]
            if kind != "anim":
                sys.exit(f"{item}: only animations can be chained")
            loop = None if anim_loop is None else len(steps) + anim_loop
            steps += anim
        return [(spec[-1], None, "anim", (steps, loop))]
    if isinstance(spec, dict):
        return [(spec.get("name", n), spec.get("zoom"), kind, data)
                for n, _, kind, data in resolve(images, spec["image"])]
    if spec.endswith(".png"):
        return [(Path(spec).stem, None, "choices", [spec])]
    kind, *rest = images[spec]
    if kind == "static":
        return [(spec, None, "choices", [rest[0]])]
    if kind == "pool":
        return [(spec, None, "choices", rest[0])]
    if kind == "anim":
        return [(spec, None, "anim", (rest[0], rest[1]))]
    return [layer for sub in rest[0] for layer in resolve(images, sub)]


# ---------------------------------------------------------------- pixels

def key24(rgb):
    rgb = rgb.astype(np.uint32)
    return (rgb[..., 0] << 16) | (rgb[..., 1] << 8) | rgb[..., 2]


def zoom_nearest(a, z):
    """Nearest-neighbour zoom about the centre, same size (Ren'Py `truecenter zoom z`); keeps the colour set."""
    h, w = a.shape[:2]
    ys = np.floor((np.arange(h) + 0.5 - h / 2) / z + h / 2).astype(int).clip(0, h - 1)
    xs = np.floor((np.arange(w) + 0.5 - w / 2) / z + w / 2).astype(int).clip(0, w - 1)
    return a[ys][:, xs]


class Source:
    """Game images at native resolution: 2x2 grid verified, soft alpha snapped, optionally zoomed, cropped."""

    def __init__(self, crop):
        self.crop, self.cache, self.soft, self.outside = crop, {}, 0, 0

    def __call__(self, rel, zoom=None):
        if (rel, zoom) not in self.cache:
            path = next((p for p in (RIP / "frames" / "images" / rel, RIP / "frames" / rel) if p.is_file()), None)
            if path is None:
                sys.exit(f"{rel}: not found under rip/frames (run tools/rip.sh)")
            a = np.array(Image.open(path).convert("RGBA"))
            if a.shape[:2] != (NATIVE[1] * PIXEL, NATIVE[0] * PIXEL):
                sys.exit(f"{rel}: {a.shape[1]}x{a.shape[0]}, expected {NATIVE[0] * PIXEL}x{NATIVE[1] * PIXEL}")
            # Sample every PIXEL x PIXEL block, at the grid phase that fits best: some cg_mirror_gg stills are drawn one
            # full-resolution pixel off the grid. What still doesn't fit (a seam column, stray touches, the 1 px outlines
            # of the gg4 reflections at up to ~4%) keeps each block's top-left pixel.
            best = None
            for dy in range(PIXEL):
                for dx in range(PIXEL):
                    shifted = np.roll(a, (-dy, -dx), (0, 1))
                    sample = shifted[::PIXEL, ::PIXEL]
                    off = int((np.repeat(np.repeat(sample, PIXEL, 0), PIXEL, 1) != shifted).any(-1).sum())
                    if best is None or off < best[0]:
                        best = (off, (dx, dy), sample.copy())
                    if off == 0:
                        break
                if best[0] == 0:
                    break
            off, phase, n = best
            if off > MAX_OFF_GRID * a.shape[0] * a.shape[1]:
                sys.exit(f"{rel}: {off} px off the {PIXEL}x{PIXEL} pixel grid")
            if off or phase != (0, 0):
                print(f"note: {rel}: {off} px off the {PIXEL}x{PIXEL} grid, sampled at phase {phase}", file=sys.stderr)
            self.soft += int(((n[..., 3] > 0) & (n[..., 3] < 255)).sum())
            n[..., 3] = np.where(n[..., 3] >= 128, 255, 0)
            if zoom:
                n = zoom_nearest(n, zoom)
            x0, y0, x1, y1 = self.crop
            content = (n[..., 3] == 255) & (key24(n[..., :3]) != key24(np.array(BACKGROUND)))
            self.outside += int(content.sum() - content[y0:y1, x0:x1].sum())
            self.cache[rel, zoom] = n[y0:y1, x0:x1]
        return self.cache[rel, zoom]


# ---------------------------------------------------------------- pack

def build(scene, palettes, images, packs):
    spec = SCENES[scene]
    crop = spec.get("crop", (0, 0, *NATIVE))
    size = (crop[2] - crop[0], crop[3] - crop[1])
    layers = [(*layer, s.get("drift") if isinstance(s, dict) else None) for s in spec["layers"] for layer in resolve(images, s)]
    source = Source(crop)
    paths = lambda kind, data: data if kind == "choices" else [p for p, _ in data[0]]
    for _, zoom, kind, data, _ in layers:
        for rel in paths(kind, data):
            source(rel, zoom)

    rgba = list(source.cache.values())
    keys = np.unique(np.concatenate([key24(np.array(BACKGROUND))[None]] + [key24(a[a[..., 3] == 255][:, :3]) for a in rgba]))
    if len(keys) > 255:
        sys.exit(f"{scene}: {len(keys)} colours, the index space holds 255")
    colours = np.stack([(keys >> 16) & 255, (keys >> 8) & 255, keys & 255], 1).astype(np.uint8)
    plte = [0, 0, 0] + colours.ravel().tolist()
    plte += [0] * (768 - len(plte))  # a full PLTE keeps Pillow writing 8-bit indices
    background = int(np.searchsorted(keys, key24(np.array(BACKGROUND)))) + 1

    def indices(rel, zoom):
        a = source(rel, zoom)
        return np.where(a[..., 3] == 255, np.searchsorted(keys, key24(a[..., :3])) + 1, 0).astype(np.uint8)

    out = packs / scene
    shutil.rmtree(out, ignore_errors=True)
    for sub in ("images", "luts", "preview"):
        (out / sub).mkdir(parents=True)
    written = set()

    def save(idx):
        name = hashlib.sha1(idx.tobytes() + str(idx.shape).encode()).hexdigest()[:16]
        rel = f"images/{name}.png"
        if rel not in written:
            im = Image.fromarray(np.ascontiguousarray(idx), "P")
            im.putpalette(plte)
            im.save(out / rel, transparency=0)
            written.add(rel)
        return rel

    def patch(a, b):
        diff = a != b
        ys, xs = np.nonzero(diff)
        if not len(ys):
            return None
        x0, y0, x1, y1 = int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1
        return {"rect": [x0, y0, x1, y1], "image": save(b[y0:y1, x0:x1]), "dirty": dirty_rects(diff)}

    def dirty_rects(diff):
        """Changed pixels as rectangles on a TILE grid: runs of changed tiles in each tile row, stacked while the
        same run continues in the next row. Shadows moving all over the window change ~17% of it, not the 99% of
        their bounding box."""
        h, w = diff.shape
        th, tw = -(-h // TILE), -(-w // TILE)
        padded = np.zeros((th * TILE, tw * TILE), bool)
        padded[:h, :w] = diff
        tiles = padded.reshape(th, TILE, tw, TILE).any((1, 3))
        rects, running = [], {}  # (tx0, tx1) -> first tile row
        for ty in range(th + 1):
            runs = set()
            if ty < th:
                edges = np.flatnonzero(np.diff(np.concatenate([[0], tiles[ty].astype(np.int8), [0]])))
                runs = {(int(s), int(e)) for s, e in zip(edges[::2], edges[1::2], strict=True)}
            for run in [r for r in running if r not in runs]:
                rects.append((running.pop(run), run, ty))
            for run in runs:
                running.setdefault(run, ty)
        return sorted([tx0 * TILE, ty0 * TILE, min(tx1 * TILE, w), min(ty1 * TILE, h)] for ty0, (tx0, tx1), ty1 in rects)

    manifest_layers, firsts, largest = [], [], 0
    for name, zoom, kind, data, drift in layers:
        if kind == "choices":
            manifest_layers.append({"name": name, "choices": [save(indices(rel, zoom)) for rel in data], "sources": data,
                                    **({"drift": parse_transform(drift)} if drift else {})})
            firsts.append(indices(data[0], zoom))
            continue
        steps, loop = data
        frames, remap = [], []  # [indices, holds]; consecutive identical images merge their holds
        for rel, holds in steps:
            idx = indices(rel, zoom)
            if frames and np.array_equal(frames[-1][0], idx):
                frames[-1][1] = tuple(round(a + b, 4) for a in frames[-1][1] for b in holds)
            else:
                frames.append([idx, holds])
            remap.append(len(frames) - 1)
        firsts.append(frames[0][0])
        if len(frames) == 1:
            manifest_layers.append({"name": name, "choices": [save(frames[0][0])], "sources": [steps[0][0]]})
            continue
        loop = None if loop is None else remap[loop]
        layer = {"name": name, "base": save(frames[0][0]),
                 "steps": [{"hold": sorted(set(h)), "patch": patch(frames[i - 1][0], f) if i else None}
                           for i, (f, h) in enumerate(frames)],
                 "loop": loop, "wrap": None if loop is None else patch(frames[-1][0], frames[loop][0])}
        for p in [s["patch"] for s in layer["steps"]] + [layer["wrap"]]:
            if p:
                x0, y0, x1, y1 = p["rect"]
                largest = max(largest, (x1 - x0) * (y1 - y0))
        manifest_layers.append(layer)

    composite = np.full(size[::-1], background, np.uint8)
    for idx in firsts:
        composite = np.where(idx != 0, idx, composite)
    lut_paths = {}
    for name in palettes:
        try:
            mapped = load_palette(name, PALETTES).map(colours)
        except LookupError as e:
            sys.exit(str(e))
        file = name
        lut = np.zeros((256, 4), np.uint8)
        lut[1:len(mapped) + 1] = np.column_stack([mapped[:, 2], mapped[:, 1], mapped[:, 0], np.full(len(mapped), 255)])
        (out / "luts" / f"{file}.bin").write_bytes(lut.tobytes())
        lut_paths[file] = f"luts/{file}.bin"
        rgb = np.zeros((256, 3), np.uint8)
        rgb[1:len(mapped) + 1] = mapped
        Image.fromarray(rgb[composite], "RGB").save(out / "preview" / f"{file}.png")

    manifest = {"version": 1, "scene": scene, "size": list(size), "crop": list(crop), "background": background,
                "colours": ["#" + bytes(c.tolist()).hex() for c in colours], "palettes": lut_paths,
                "layers": manifest_layers}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n")
    if source.outside:
        print(f"warning: {scene}: {source.outside} content px fall outside the crop", file=sys.stderr)
    print(f"{scene}: {size[0]}x{size[1]}, {len(manifest_layers)} layers, {len(written)} images, {len(keys)} colours, "
          f"largest patch {largest / (size[0] * size[1]):.1%} of the canvas, {source.soft} soft-alpha px snapped")


def main():
    scenes = "\n".join(textwrap.wrap(", ".join(sorted(SCENES)), 100, initial_indent="  ", subsequent_indent="  "))
    ap = argparse.ArgumentParser(
        prog="molokolive-pack",
        description="Build the scene packs molokolive plays, from the game files tools/rip.sh extracted.",
        epilog=f"""examples:
  molokolive-pack                          build every scene
  molokolive-pack cg_floor mini_cg_run     rebuild two scenes

scenes:
{scenes}""",
        formatter_class=HelpFormatter,
    )
    ap.add_argument("scenes", nargs="*", metavar="SCENE", help="scenes to build (default: all)")
    palettes = ["neutral-lift", "firefly-neutral", "none"]
    ap.add_argument("--palette", nargs="+", metavar="NAME", default=palettes,
                    help=f"palettes to include, from palettes/ (default: {' '.join(palettes)})")
    ap.add_argument("--packs", type=Path, metavar="DIR", default=default_packs(),
                    help="where to write the packs (default: ~/.local/share/molokolive/packs)")
    args = ap.parse_args()
    if unknown := [s for s in args.scenes if s not in SCENES]:
        ap.error(f"unknown scene {', '.join(unknown)} (the list is in --help)")
    images = parse_images(RIP / "raw" / "art.rpy")
    for scene in args.scenes or sorted(SCENES):
        build(scene, args.palette, images, args.packs)


if __name__ == "__main__":
    main()

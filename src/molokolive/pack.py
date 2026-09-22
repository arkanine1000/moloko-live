"""Build scene packs for the wallpaper engine (engine/).

  molokolive-pack                                        # every scene
  molokolive-pack mini_cg_run cg_floor --palette firefly-neutral

scenes.py lists each scene as the layer stack rip/raw/script.rpy shows. The images themselves (plain paths,
Animation(...), ATL blocks with choice/pause/repeat, LiveComposite) are parsed from rip/raw/art.rpy by renpy.py.

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
import shutil
import sys
import textwrap
from pathlib import Path

import numpy as np
from PIL import Image

from molokolive import scenes
from molokolive.cli import HelpFormatter, ToolError, run
from molokolive.colour import load_palette, rgb_to_hex
from molokolive.paths import PALETTES, RIP, default_packs
from molokolive.pixels import bounding_box, dirty_rects, key24, sample_grid, zoom_nearest
from molokolive.renpy import parse_images, parse_transform
from molokolive.scenes import BACKGROUND, MAX_OFF_GRID, NATIVE, PIXEL, SCENES, TILE, Choices


class Source:
    """Game images at native resolution: sampled off the 2x2 grid, soft alpha snapped, optionally zoomed, cropped.
    Counts the soft-alpha pixels snapped and the content pixels lost to the crop."""

    def __init__(self, rip, crop):
        self.rip, self.crop, self.cache, self.soft, self.outside = rip, crop, {}, 0, 0

    def __call__(self, rel, zoom=None):
        if (rel, zoom) not in self.cache:
            self.cache[rel, zoom] = self.load(rel, zoom)
        return self.cache[rel, zoom]

    def load(self, rel, zoom):
        path = next((p for p in (self.rip / "frames" / "images" / rel, self.rip / "frames" / rel) if p.is_file()), None)
        if path is None:
            raise ToolError(f"{rel}: not found under rip/frames (run tools/rip.sh)")
        a = np.array(Image.open(path).convert("RGBA"))
        if a.shape[:2] != (NATIVE[1] * PIXEL, NATIVE[0] * PIXEL):
            raise ToolError(f"{rel}: {a.shape[1]}x{a.shape[0]}, expected {NATIVE[0] * PIXEL}x{NATIVE[1] * PIXEL}")
        # Some stills are drawn one full-resolution pixel off the grid, so the phase is searched per image.
        n, off, phase = sample_grid(a, PIXEL)
        if off > MAX_OFF_GRID * a.shape[0] * a.shape[1]:
            raise ToolError(f"{rel}: {off} px off the {PIXEL}x{PIXEL} pixel grid")
        if off or phase != (0, 0):
            print(f"note: {rel}: {off} px off the {PIXEL}x{PIXEL} grid, sampled at phase {phase}", file=sys.stderr)
        self.soft += int(((n[..., 3] > 0) & (n[..., 3] < 255)).sum())
        n[..., 3] = np.where(n[..., 3] >= 128, 255, 0)
        if zoom:
            n = zoom_nearest(n, zoom)
        x0, y0, x1, y1 = self.crop
        content = (n[..., 3] == 255) & (key24(n[..., :3]) != key24(np.array(BACKGROUND)))
        self.outside += int(content.sum() - content[y0:y1, x0:x1].sum())
        return n[y0:y1, x0:x1]


class IndexSpace:
    """One index per colour used by any of a scene's images, plus index 0 for transparency."""

    def __init__(self, scene, images):
        self.keys = np.unique(np.concatenate([key24(np.array(BACKGROUND))[None]]
                                             + [key24(a[a[..., 3] == 255][:, :3]) for a in images]))
        if len(self.keys) > 255:
            raise ToolError(f"{scene}: {len(self.keys)} colours, the index space holds 255")
        keys = self.keys
        self.colours = np.stack([(keys >> 16) & 255, (keys >> 8) & 255, keys & 255], 1).astype(np.uint8)
        self.background = int(np.searchsorted(keys, key24(np.array(BACKGROUND)))) + 1
        self.plte = [0, 0, 0] + self.colours.ravel().tolist()
        self.plte += [0] * (768 - len(self.plte))  # a full PLTE keeps Pillow writing 8-bit indices

    def indices(self, rgba):
        """An RGBA image -> its index image, 0 where transparent."""
        return np.where(rgba[..., 3] == 255, np.searchsorted(self.keys, key24(rgba[..., :3])) + 1, 0).astype(np.uint8)


class PackDir:
    """A scene's pack directory, emptied on creation. Images are named by content, so equal images share a file."""

    def __init__(self, path, plte):
        self.path, self.plte, self.written = path, plte, set()
        shutil.rmtree(path, ignore_errors=True)
        for sub in ("images", "luts", "preview"):
            (path / sub).mkdir(parents=True)

    def save(self, idx):
        """Write an index image once; its path inside the pack."""
        name = hashlib.sha1(idx.tobytes() + str(idx.shape).encode()).hexdigest()[:16]
        rel = f"images/{name}.png"
        if rel not in self.written:
            im = Image.fromarray(np.ascontiguousarray(idx), "P")
            im.putpalette(self.plte)
            im.save(self.path / rel, transparency=0)
            self.written.add(rel)
        return rel

    def patch(self, a, b):
        """What changes from index image a to b, as a manifest patch, or None if nothing does."""
        diff = a != b
        rect = bounding_box(diff)
        if rect is None:
            return None
        x0, y0, x1, y1 = rect
        return {"rect": rect, "image": self.save(b[y0:y1, x0:x1]), "dirty": dirty_rects(diff, TILE)}


def choices_layer(pack, index, source, layer, script):
    """A Choices layer -> (its manifest entry, the index image of its first choice)."""
    entry = {"name": layer.name, "choices": [pack.save(index.indices(source(rel, layer.zoom))) for rel in layer.paths],
             "sources": layer.paths}
    if layer.drift:
        entry["drift"] = parse_transform(script, layer.drift, PIXEL)
    return entry, index.indices(source(layer.paths[0], layer.zoom))


def timeline_layer(pack, index, source, layer):
    """A Timeline layer -> (its manifest entry, the index image of its first frame, its largest patch in pixels).
    Consecutive identical frames merge into one step whose holds add up."""
    frames, remap = [], []  # [index image, holds]; remap: frame number -> step
    for rel, holds in layer.frames:
        idx = index.indices(source(rel, layer.zoom))
        if frames and np.array_equal(frames[-1][0], idx):
            frames[-1][1] = tuple(round(a + b, 4) for a in frames[-1][1] for b in holds)
        else:
            frames.append([idx, holds])
        remap.append(len(frames) - 1)
    first = frames[0][0]
    if len(frames) == 1:
        return {"name": layer.name, "choices": [pack.save(first)], "sources": [layer.frames[0][0]]}, first, 0
    loop = None if layer.loop is None else remap[layer.loop]
    entry = {"name": layer.name, "base": pack.save(first),
             "steps": [{"hold": sorted(set(h)), "patch": pack.patch(frames[i - 1][0], f) if i else None}
                       for i, (f, h) in enumerate(frames)],
             "loop": loop, "wrap": None if loop is None else pack.patch(frames[-1][0], frames[loop][0])}
    largest = 0
    for p in [s["patch"] for s in entry["steps"]] + [entry["wrap"]]:
        if p:
            x0, y0, x1, y1 = p["rect"]
            largest = max(largest, (x1 - x0) * (y1 - y0))
    return entry, first, largest


def write_luts(pack, index, composite, palettes, palettes_dir):
    """One LUT and preview per palette -> {palette: LUT path}."""
    lut_paths = {}
    for name in palettes:
        try:
            mapped = load_palette(name, palettes_dir).map(index.colours)
        except LookupError as e:
            raise ToolError(str(e)) from None
        lut = np.zeros((256, 4), np.uint8)
        lut[1:len(mapped) + 1] = np.column_stack([mapped[:, 2], mapped[:, 1], mapped[:, 0], np.full(len(mapped), 255)])
        (pack.path / "luts" / f"{name}.bin").write_bytes(lut.tobytes())
        lut_paths[name] = f"luts/{name}.bin"
        rgb = np.zeros((256, 3), np.uint8)
        rgb[1:len(mapped) + 1] = mapped
        Image.fromarray(rgb[composite], "RGB").save(pack.path / "preview" / f"{name}.png")
    return lut_paths


def build_scene(scene, palettes, images, script, rip, palettes_dir, packs):
    """Write one scene's pack and print a line about it."""
    spec = SCENES[scene]
    crop = spec.get("crop", (0, 0, *NATIVE))
    size = (crop[2] - crop[0], crop[3] - crop[1])
    layers = scenes.scene_layers(images, scene)
    source = Source(rip, crop)
    for layer in layers:
        for rel in (layer.paths if isinstance(layer, Choices) else [p for p, _ in layer.frames]):
            source(rel, layer.zoom)
    index = IndexSpace(scene, source.cache.values())
    pack = PackDir(packs / scene, index.plte)

    entries, firsts, largest = [], [], 0
    for layer in layers:
        if isinstance(layer, Choices):
            entry, first = choices_layer(pack, index, source, layer, script)
        else:
            entry, first, area = timeline_layer(pack, index, source, layer)
            largest = max(largest, area)
        entries.append(entry)
        firsts.append(first)

    composite = np.full(size[::-1], index.background, np.uint8)
    for idx in firsts:
        composite = np.where(idx != 0, idx, composite)
    lut_paths = write_luts(pack, index, composite, palettes, palettes_dir)

    manifest = {"version": 1, "scene": scene, "size": list(size), "crop": list(crop), "background": index.background,
                "colours": [rgb_to_hex(c) for c in index.colours], "palettes": lut_paths, "layers": entries}
    (pack.path / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n")
    if source.outside:
        print(f"warning: {scene}: {source.outside} content px fall outside the crop", file=sys.stderr)
    print(f"{scene}: {size[0]}x{size[1]}, {len(entries)} layers, {len(pack.written)} images, {len(index.keys)} colours, "
          f"largest patch {largest / (size[0] * size[1]):.1%} of the canvas, {source.soft} soft-alpha px snapped")


def command():
    names = "\n".join(textwrap.wrap(", ".join(sorted(SCENES)), 100, initial_indent="  ", subsequent_indent="  "))
    ap = argparse.ArgumentParser(
        prog="molokolive-pack",
        description="Build the scene packs molokolive plays, from the game files tools/rip.sh extracted.",
        epilog=f"""examples:
  molokolive-pack                          build every scene
  molokolive-pack cg_floor mini_cg_run     rebuild two scenes

scenes:
{names}""",
        formatter_class=HelpFormatter,
    )
    ap.add_argument("scenes", nargs="*", metavar="SCENE", help="scenes to build (default: all)")
    palettes = ["neutral-lift", "firefly-neutral", "none"]
    ap.add_argument("--palette", nargs="+", metavar="NAME", default=palettes,
                    help=f"palettes to include, from palettes/ (default: {' '.join(palettes)})")
    ap.add_argument("--packs", type=Path, metavar="DIR", default=default_packs(),
                    help="where to write the packs (default: ~/.local/share/molokolive/packs)")
    ap.add_argument("--rip", type=Path, metavar="DIR", default=RIP, help="the extracted game files (default: rip/)")
    ap.add_argument("--palettes", type=Path, metavar="DIR", default=PALETTES, help="palette files (default: palettes/)")
    args = ap.parse_args()
    if unknown := [s for s in args.scenes if s not in SCENES]:
        ap.error(f"unknown scene {', '.join(unknown)} (the list is in --help)")
    try:
        images = parse_images((args.rip / "raw" / "art.rpy").read_text(encoding="utf-8"))
        script = (args.rip / "raw" / "script.rpy").read_text(encoding="utf-8")
    except FileNotFoundError as e:
        raise ToolError(f"{e.filename}: not found (run tools/rip.sh, or pass --rip)") from None
    for scene in args.scenes or sorted(SCENES):
        build_scene(scene, args.palette, images, script, args.rip, args.palettes, args.packs)


def main():
    run(command)


if __name__ == "__main__":
    main()
